import { expect, test, type Page } from "@playwright/test";

const ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SECRET = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const INITIAL = {
  id: ID, name: "orders", image: "ghcr.io/team/orders:1.0", runtime: "vm",
  port: 8080, healthPath: "/actuator/health", domain: "orders.example.com",
  ram: 1024, cpu: 2, replicas: 3, startupTimeout: 120, drainTimeout: 60,
  env: { PROFILE: "prod" }, secrets: { DB_PASSWORD: SECRET },
};

async function openServices(page: Page, rows: object[] = [], failSave = false) {
  const writes: { method: string; body: Record<string, unknown> }[] = [];
  let saved = rows;
  await page.route(/^https?:\/\/[^/]+\/api\//, async (route) => {
    const path = new URL(route.request().url()).pathname;
    const method = route.request().method();
    if (path === "/api/vms") return route.fulfill({ json: [] });
    if (path === "/api/update") return route.fulfill({ json: { current: "0.3.1", latest: "0.3.1", updateAvailable: false } });
    if (path.startsWith("/api/micro-services")) {
      if (method === "POST" || method === "PUT") {
        const body = route.request().postDataJSON();
        writes.push({ method, body });
        if (failSave) return route.fulfill({ status: 400, json: { error: { code: "validation_failed", message: "request validation failed", fields: { name: "is already registered" } } } });
        saved = [{ id: ID, ...body }];
        return route.fulfill({ status: method === "POST" ? 201 : 200, json: saved[0] });
      }
      if (method === "DELETE") { saved = []; return route.fulfill({ status: 204 }); }
      return route.fulfill({ json: path === "/api/micro-services" ? saved : saved[0] });
    }
    return route.fulfill({ status: 404, json: { error: { message: "unused endpoint" } } });
  });
  await page.addInitScript(() => localStorage.setItem("firecrab.locale", "en"));
  await page.goto("/#/services");
  return writes;
}

test.describe("MicroService configuration @dashboard", () => {
  const browserErrors = new WeakMap<Page, string[]>();
  test.beforeEach(({ page }) => {
    const errors: string[] = [];
    browserErrors.set(page, errors);
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => {
      // The validation test intentionally receives this HTTP error.
      const expectedValidation = "Failed to load resource: the server responded with a status of 400 (Bad Request)";
      if (message.type() === "error" && message.text() !== expectedValidation) errors.push(message.text());
    });
  });
  test.afterEach(({ page }) => expect(browserErrors.get(page)).toEqual([]));
  test("creates, reads, edits and deletes configurations", async ({ page }, testInfo) => {
    const writes = await openServices(page);
    await expect(page.getByText("No service configurations yet.")).toBeVisible();
    await page.getByRole("button", { name: "New MicroService", exact: true }).click();
    await page.getByLabel("Name", { exact: true }).fill("orders");
    await page.getByLabel("OCI image", { exact: true }).fill("ghcr.io/team/orders:1.0");
    await page.getByRole("button", { name: "Add variable", exact: true }).click();
    await page.locator("#service-env-key-0").fill("PROFILE");
    await page.locator("#service-env-value-0").fill("prod");
    await page.getByRole("button", { name: "Add secret reference", exact: true }).click();
    await page.locator("#service-secrets-key-0").fill("DB_PASSWORD");
    await page.locator("#service-secrets-value-0").fill(SECRET);
    await page.screenshot({ path: testInfo.outputPath("microservice-desktop.png"), fullPage: true });
    await page.getByRole("button", { name: "Save configuration" }).click();
    await expect(page.getByRole("cell", { name: "orders", exact: true })).toBeVisible();
    expect(writes[0]).toEqual({ method: "POST", body: {
      name: "orders", image: "ghcr.io/team/orders:1.0", runtime: "vm", port: 8080,
      healthPath: "/health", domain: null, ram: 512, cpu: 1, replicas: 1,
      startupTimeout: 300, drainTimeout: 30, env: { PROFILE: "prod" }, secrets: { DB_PASSWORD: SECRET },
    } });

    await page.getByRole("button", { name: "Edit orders", exact: true }).click();
    await expect(page.locator("#service-secrets-value-0")).toHaveValue(SECRET);
    await page.getByLabel("OCI image", { exact: true }).fill("ghcr.io/team/orders:2.0");
    await page.getByLabel("Replicas", { exact: true }).fill("2");
    await page.getByRole("button", { name: "Save configuration" }).click();
    await expect(page.getByRole("cell", { name: "ghcr.io/team/orders:2.0", exact: true })).toBeVisible();
    expect(writes[1].method).toBe("PUT");
    expect(writes[1].body).not.toHaveProperty("id");
    expect(writes[1].body.replicas).toBe(2);
    page.once("dialog", (dialog) => dialog.accept());
    await page.getByRole("button", { name: "Delete orders", exact: true }).click();
    await expect(page.getByText("No service configurations yet.")).toBeVisible();
  });

  test("keeps edits after validation failure and cancels without saving", async ({ page }) => {
    const writes = await openServices(page, [INITIAL], true);
    await page.getByRole("button", { name: "Edit orders", exact: true }).click();
    await expect(page.getByLabel("Health path", { exact: true })).toHaveValue("/actuator/health");
    await expect(page.getByLabel("Startup timeout (s)")).toHaveValue("120");
    await expect(page.getByLabel("Drain timeout (s)")).toHaveValue("60");
    await page.getByLabel("Name", { exact: true }).fill("billing");
    await page.getByRole("button", { name: "Save configuration" }).click();
    await expect(page.getByText("is already registered", { exact: true })).toBeVisible();
    await expect(page.getByLabel("Name", { exact: true })).toHaveValue("billing");
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(page.getByRole("cell", { name: "orders", exact: true })).toBeVisible();
    expect(writes).toHaveLength(1);
  });

  test("supports Korean and mobile editing without page overflow", async ({ page }, testInfo) => {
    await openServices(page, [INITIAL]);
    await page.getByRole("combobox", { name: "Language", exact: true }).selectOption("ko");
    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole("button", { name: "orders 수정", exact: true }).click();
    await expect(page.getByRole("heading", { name: "수정: orders", exact: true })).toBeVisible();
    await expect(page.getByLabel("상태 검사 경로")).toHaveValue("/actuator/health");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect(await page.locator(".table-scroll").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath("microservice-mobile.png"), fullPage: true });
  });
});
