import { expect, test, type Page } from "@playwright/test";

const initial = {
  config: { listen: "127.0.0.1:8080", routes: [{ host: "app.example.com", pathPrefix: "/", upstreams: ["127.0.0.1:18081", "127.0.0.1:18082"] }], tunnel: null },
  units: [
    { unit: "firecrab-ingress.service", loadState: "loaded", activeState: "active", subState: "running", mainPid: 101 },
    { unit: "firecrab-tunnel.service", loadState: "not-found", activeState: "inactive", subState: "dead", mainPid: 0 },
  ],
  nginxAvailable: true, cloudflaredAvailable: true,
};

async function open(page: Page, options: { unavailable?: boolean; locale?: string; reject?: boolean } = {}) {
  let snapshot = structuredClone(initial);
  const writes: { config: typeof initial.config; credentials: unknown }[] = [];
  let unavailable = options.unavailable ?? false;
  await page.route(/^https?:\/\/[^/]+\/api\//, async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/vms") return route.fulfill({ json: [] });
    if (path === "/api/update") return route.fulfill({ json: { current: "0.3.1", latest: "0.3.1", updateAvailable: false } });
    if (path === "/api/pipe/disable") { snapshot.units.forEach(unit => { unit.activeState = "inactive"; unit.mainPid = 0; }); return route.fulfill({ json: snapshot }); }
    if (path === "/api/pipe") {
      if (unavailable) return route.fulfill({ status: 503, json: { error: { message: "Pipe helper unavailable" } } });
      if (route.request().method() === "PUT") {
        const body = route.request().postDataJSON();
        writes.push(body);
        if (options.reject) return route.fulfill({ status: 400, json: { error: { message: "request validation failed", fields: { config: "invalid app address" } } } });
        snapshot = { ...snapshot, config: body.config };
      }
      return route.fulfill({ json: snapshot });
    }
    return route.fulfill({ status: 404 });
  });
  await page.addInitScript(locale => localStorage.setItem("firecrab.locale", locale), options.locale ?? "en");
  await page.goto("/#/pipe");
  return { writes, unavailable: () => { unavailable = true; } };
}

test.describe("Pipe dashboard @dashboard", () => {
  test("edits domain/path routes and applies multiple app addresses", async ({ page }, info) => {
    const mock = await open(page);
    await expect(page.getByRole("button", { name: "Pipe", exact: true })).toHaveAttribute("aria-current", "page");
    await expect(page.getByRole("region", { name: "Pipe", exact: true })).toContainText("active");
    await page.getByLabel("Domain", { exact: true }).fill("api.example.com");
    await page.getByLabel("Path prefix", { exact: true }).fill("/api");
    await page.getByLabel("App addresses", { exact: true }).fill("127.0.0.1:18083, 127.0.0.1:18084");
    await page.getByRole("button", { name: "Add route", exact: true }).click();
    await expect(page.getByText("Unapplied changes")).toBeVisible();
    await page.getByRole("button", { name: "Save and apply", exact: true }).click();
    await expect(page.getByText("Routes applied.", { exact: false })).toBeVisible();
    expect(mock.writes[0].config.routes[1]).toEqual({ host: "api.example.com", pathPrefix: "/api", upstreams: ["127.0.0.1:18083", "127.0.0.1:18084"] });
    expect(mock.writes[0].credentials).toBeNull();
    await page.screenshot({ path: info.outputPath("pipe-desktop.png"), fullPage: true });
    await page.getByRole("row").filter({ hasText: "api.example.com" }).getByRole("button", { name: "Remove" }).click();
    await page.getByRole("button", { name: "Save and apply", exact: true }).click();
    await expect.poll(() => mock.writes.length).toBe(2);
    expect(mock.writes[1].config.routes).toHaveLength(1);
  });

  test("keeps edits when apply fails and clears stale service states", async ({ page }) => {
    await page.clock.install();
    const mock = await open(page, { reject: true });
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page.getByLabel("App addresses", { exact: true }).fill("127.0.0.1:19001");
    await page.getByRole("button", { name: "Update route", exact: true }).click();
    await page.getByRole("button", { name: "Save and apply", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("invalid app address");
    await expect(page.getByRole("region", { name: "Routes", exact: true })).toContainText("127.0.0.1:19001");
    mock.unavailable(); await page.clock.runFor(8_100);
    const panel = page.getByRole("region", { name: "Pipe", exact: true });
    await expect(panel).toContainText("Unavailable");
    await expect(panel.locator(".state-badge.running")).toHaveCount(0);
  });

  test("uses the existing responsive shell and clears credentials after saving", async ({ page }, info) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const mock = await open(page, { locale: "ko" });
    await page.getByRole("checkbox").check();
    await page.getByLabel("Tunnel UUID").fill("11111111-1111-4111-8111-111111111111");
    const credentials = { AccountTag: "fixture", TunnelID: "11111111-1111-4111-8111-111111111111", TunnelSecret: "private-fixture" };
    await page.getByLabel("인증 정보 JSON", { exact: true }).fill(JSON.stringify(credentials));
    await page.getByRole("button", { name: "저장·적용" }).click();
    await expect(page.getByLabel("인증 정보 JSON", { exact: true })).toHaveValue("");
    expect(mock.writes[0].credentials).toEqual(credentials);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath("pipe-mobile-ko.png"), fullPage: true });
    page.on("dialog", dialog => dialog.accept());
    await page.getByRole("button", { name: "중지", exact: true }).click();
    await expect(page.getByText("Pipe를 중지했다.", { exact: false })).toBeVisible();
    await expect(page.locator(".state-badge.running")).toHaveCount(0);
  });
});
