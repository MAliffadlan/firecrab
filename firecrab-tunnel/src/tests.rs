use super::*;
use axum::{
    body::to_bytes,
    extract::ws::{CloseFrame, Message, WebSocketUpgrade},
    routing::{any, get},
};
use futures_util::{SinkExt, StreamExt};
use tower::ServiceExt;

const OWNER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
fn request(method: &str, name: &str, owner: &str, target: Option<Target>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!("/v1/addresses/{name}"))
        .header("host", "tunnel.firecrab.dev")
        .header("authorization", format!("Bearer {owner}"))
        .body(Body::from(
            serde_json::json!({"enabled":target.is_some(),"target":target}).to_string(),
        ))
        .unwrap()
}
fn target(url: &str) -> Option<Target> {
    Some(Target {
        kind: Kind::Nginx,
        url: url.into(),
    })
}

#[tokio::test]
async fn ownership_is_durable_idempotent_and_cannot_be_replaced_or_deleted_by_another_host() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("registry.db");
    let service = Service::open(
        &path,
        "firecrab.dev".into(),
        vec!["https://nginx.example.com".into()],
    )
    .unwrap();
    let app = service.clone().router();
    for _ in 0..2 {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "PUT",
                    "my-app",
                    OWNER,
                    target("https://nginx.example.com")
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    let other = "a".repeat(64);
    for method in ["PUT", "DELETE"] {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    method,
                    "my-app",
                    &other,
                    target("https://nginx.example.com")
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }
    drop(app);
    drop(service);
    let reopened = Service::open(&path, "firecrab.dev".into(), vec![]).unwrap();
    assert!(reopened.binding("my-app").unwrap().is_some());
    let app = reopened.router();
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", "my-app.firecrab.dev")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "PUT",
                "my-app",
                OWNER,
                Some(Target {
                    kind: Kind::QuickTunnel,
                    url: "https://blue-crab.trycloudflare.com".into()
                })
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone()
            .oneshot(request("DELETE", "my-app", OWNER, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.oneshot(request("DELETE", "my-app", OWNER, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn rejects_reserved_names_credentials_private_targets_and_url_injection() {
    let directory = tempfile::tempdir().unwrap();
    let service =
        Service::open(&directory.path().join("db"), "firecrab.dev".into(), vec![]).unwrap();
    for invalid in [
        "https://user@crab.trycloudflare.com",
        "https://crab.trycloudflare.com/secret",
        "https://crab.trycloudflare.com.evil.test",
        "https://a.b.trycloudflare.com",
        "http://crab.trycloudflare.com",
        "http://127.0.0.1:5523",
        "https://169.254.169.254",
        "https://crab.trycloudflare.com?foo=bar",
    ] {
        assert!(
            !service.valid_target(&Target {
                kind: Kind::QuickTunnel,
                url: invalid.into()
            }),
            "{invalid}"
        );
        assert!(
            !service.valid_target(&Target {
                kind: Kind::Nginx,
                url: invalid.into()
            }),
            "{invalid}"
        );
    }
    let app = service.router();
    assert_eq!(
        app.clone()
            .oneshot(request("PUT", "my-app", "short", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for name in [
        "api",
        "www",
        "tunnel",
        "a.b",
        "-bad",
        "UPPER",
        "my-app/extra",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request("PUT", name, OWNER, None))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let huge = Request::builder()
        .method("PUT")
        .uri("/v1/addresses/my-app")
        .header("host", "tunnel.firecrab.dev")
        .header("authorization", format!("Bearer {OWNER}"))
        .body(Body::from("x".repeat(4097)))
        .unwrap();
    assert_eq!(
        app.oneshot(huge).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn real_http_proxy_preserves_paths_body_headers_redirects_and_offline_leases() {
    let backend = Router::new().fallback(any(|request: Request<Body>| async move {
        let (parts, body) = request.into_parts();
        if parts.uri.path() == "/redirect" {
            let host = parts.headers["host"].to_str().unwrap();
            return Response::builder()
                .status(StatusCode::FOUND)
                .header("location", format!("http://{host}/landing?from=app"))
                .body(Body::empty())
                .unwrap();
        }
        let body = to_bytes(body, 4096).await.unwrap();
        axum::Json(serde_json::json!({
            "path": parts.uri.to_string(),
            "method": parts.method.as_str(),
            "body": String::from_utf8(body.to_vec()).unwrap(),
            "host": parts.headers["x-forwarded-host"].to_str().unwrap(),
            "proto": parts.headers["x-forwarded-proto"].to_str().unwrap()
        }))
        .into_response()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let service = Service::open(
        &directory.path().join("db"),
        "firecrab.dev".into(),
        vec![origin.clone()],
    )
    .unwrap();
    let app = service.clone().router();
    assert_eq!(
        app.clone()
            .oneshot(request("PUT", "my-app", OWNER, target(&origin)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/items?q=one%20two")
                .header("host", "my-app.firecrab.dev")
                .header("x-forwarded-host", "spoofed.test")
                .body(Body::from("payload"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"path":"/api/items?q=one%20two","method":"POST","body":"payload","host":"my-app.firecrab.dev","proto":"https"})
    );
    let redirect = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/redirect")
                .header("host", "my-app.firecrab.dev")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(redirect.status(), StatusCode::FOUND);
    assert_eq!(
        redirect.headers()["location"],
        "https://my-app.firecrab.dev/landing?from=app"
    );
    service
        .db
        .lock()
        .unwrap()
        .execute("UPDATE addresses SET seen=0", [])
        .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "my-app.firecrab.dev")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    task.abort();
}

#[tokio::test]
async fn websocket_messages_cross_the_public_subdomain_proxy() {
    let backend = Router::new().route(
        "/ws",
        get(|ws: WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket| async move {
                while let Some(Ok(message)) = socket.recv().await {
                    if matches!(message, Message::Close(_)) {
                        break;
                    }
                    if matches!(&message, Message::Text(text) if text.as_str() == "close") {
                        socket
                            .send(Message::Close(Some(CloseFrame {
                                code: 1008,
                                reason: "policy".into(),
                            })))
                            .await
                            .unwrap();
                        break;
                    }
                    if socket.send(message).await.is_err() {
                        break;
                    }
                }
            })
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let backend_task = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let service = Service::open(
        &directory.path().join("db"),
        "firecrab.dev".into(),
        vec![origin.clone()],
    )
    .unwrap();
    let app = service.router();
    assert_eq!(
        app.clone()
            .oneshot(request("PUT", "my-app", OWNER, target(&origin)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let proxy_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("host", "my-app.firecrab.dev".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "hello".into(),
        ))
        .await
        .unwrap();
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(message.to_text().unwrap(), "hello");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "close".into(),
        ))
        .await
        .unwrap();
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = message else {
        panic!("upstream close frame was not forwarded");
    };
    assert_eq!(u16::from(frame.code), 1008);
    assert_eq!(frame.reason.as_str(), "policy");
    proxy_task.abort();
    backend_task.abort();
}
