// Calls on a phone (390 px): the calls page fits the screen, and the call
// screen uses the whole width with its controls at the bottom, in reach.
import { expect, test, type Page } from "@playwright/test";
import en from "../src/locales/en.js";

test.use({ viewport: { width: 390, height: 844 } });

async function fitsTheScreen(page: Page): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
}

test("on a phone the calls page lists a call, and the call fills the screen with its controls at the bottom", async ({ page, browser }) => {
  const room = `phone-check-${Date.now()}`;
  await page.goto(`/calls?room=${room}`);
  const olena = page.locator("ortus-call");
  await olena.getByRole("button", { name: en.startCall }).click();
  const leave = olena.getByRole("button", { name: en.leave });
  await expect(leave).toBeVisible();
  await fitsTheScreen(page);

  // The stage takes the app's width (less its margins).
  const app = (await olena.boundingBox())!;
  const stage = (await olena.locator("section.stage").boundingBox())!;
  expect(stage.width).toBeGreaterThan(app.width - 32);
  // Every control is on the screen, at the bottom, and big enough to tap.
  const bar = (await olena.getByRole("toolbar").boundingBox())!;
  expect(bar.y + bar.height).toBeLessThanOrEqual(844);
  expect(bar.y).toBeGreaterThan(844 / 2);
  for (const button of await olena.getByRole("toolbar").getByRole("button").all()) {
    const box = (await button.boundingBox())!;
    expect(box.height).toBeGreaterThanOrEqual(44);
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(390);
    expect(box.y + box.height).toBeLessThanOrEqual(844);
  }
  await expect(olena.getByRole("link", { name: en.backToChannel })).toBeVisible();
  await page.screenshot({ path: "test-results/phone-call.png" });

  // The calls page in another tab of hers: the call, to join.
  const other = await (await browser.newContext({ viewport: { width: 390, height: 844 } })).newPage();
  await other.goto("/calls");
  const home = other.locator("ortus-calls-home");
  const live = home.getByRole("region", { name: en.happeningNow }).getByRole("listitem").filter({ hasText: room });
  const join = live.getByRole("link", { name: en.joinCall });
  await expect(join).toBeVisible();
  expect((await join.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await fitsTheScreen(other);
  await other.screenshot({ path: "test-results/phone-calls.png" });

  // Into the call and back to the list.
  await join.click();
  await expect(other.locator("ortus-call").getByRole("button", { name: en.joinCall })).toBeVisible();
  await fitsTheScreen(other);
  await other.goBack();
  await expect(join).toBeVisible();

  await leave.click();
  await other.close();
});
