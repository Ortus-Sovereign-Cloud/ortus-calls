// The Calls page without a room: the calls going on in one's rooms, to
// join (and the start page's card says the same).
import { expect, test } from "@playwright/test";
import en from "../src/locales/en.js";

test("the calls page lists a call going on, to join", async ({ page, browser }) => {
  const room = `bridge-check-${Date.now()}`;
  // Olena starts a call in the room.
  await page.goto(`/calls?room=${room}`);
  await page.locator("ortus-call").getByRole("button", { name: en.startCall }).click();
  await expect(page.locator("ortus-call").getByRole("button", { name: en.leave })).toBeVisible();

  // The calls page, in another tab of hers: the call is happening now.
  const other = await (await browser.newContext()).newPage();
  await other.goto("/calls");
  const home = other.locator("ortus-calls-home");
  await expect(home.getByRole("heading", { level: 1, name: en.calls })).toBeVisible();
  const live = home.getByRole("region", { name: en.happeningNow }).getByRole("listitem").filter({ hasText: room });
  await expect(live).toContainText(en.inCall.one.replace("{count}", "1"));
  await expect(live.getByRole("link", { name: en.joinCall })).toHaveAttribute("href", `/calls?room=${room}`);

  // The start page's card says the same.
  await other.evaluate(async () => {
    const card = "/assets/apps/calls/today.js";
    await import(/* @vite-ignore */ card);
    document.body.append(document.createElement("ortus-calls-today"));
  });
  const card = other.locator("ortus-calls-today").getByRole("region", { name: en.happeningNow });
  await expect(card.getByRole("listitem").filter({ hasText: room }).getByRole("link", { name: en.join })).toHaveAttribute("href", `/calls?room=${room}`);

  // She leaves: nothing is happening in that room any more.
  await page.locator("ortus-call").getByRole("button", { name: en.leave }).click();
  await other.reload();
  await expect(other.locator("ortus-calls-home").getByText(room)).toHaveCount(0);
});
