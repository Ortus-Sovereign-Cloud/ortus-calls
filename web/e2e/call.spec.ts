// Two people in a room's call: each sees and hears the other through the
// media node, sees them mute and raise a hand, and sees a shared screen.
import { expect, test, type Browser, type Page } from "@playwright/test";
import en from "../src/locales/en.js";

const headers = { "X-Requested-With": "ortus" };

async function newcomer(owner: Page, name: string, email: string, browser: Browser): Promise<Page> {
  const r = await owner.request.post("/api/v1/admin/users", { headers, data: { email, name } });
  expect(r.status()).toBe(200);
  const link = new URL((await r.json()).link);
  const page = await (await browser.newContext({ storageState: { cookies: [], origins: [] }, permissions: ["camera", "microphone"] })).newPage();
  await page.goto(link.pathname + link.hash);
  await expect(page).toHaveURL(/\/welcome$/);
  return page;
}

/** Whether a tile or the stage shows moving video. */
async function playing(page: Page, selector: string): Promise<boolean> {
  return page.locator("ortus-call").locator(selector).first().evaluate((v: HTMLVideoElement) => v.videoWidth > 0 && v.readyState >= 2);
}

test("two people meet in a room's call", async ({ page, browser }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const room = `flood-response-${Date.now()}`;
  const marek = await newcomer(page, "Marek Nowak", `marek-${Date.now()}@ministry.example`, browser);
  marek.on("pageerror", (e) => errors.push(e.message));

  await page.goto(`/calls?room=${room}`);
  const olena = page.locator("ortus-call");
  await olena.getByRole("button", { name: en.startCall }).click();
  await expect(olena.getByRole("button", { name: en.leave })).toBeVisible();

  await marek.goto(`/calls?room=${room}`);
  const his = marek.locator("ortus-call");
  await his.getByRole("button", { name: en.joinCall }).click();
  await expect(his.getByText(en.inCall.other.replace("{count}", "2"))).toBeVisible();

  // Each sees the other's camera through the media node.
  await expect(his.getByRole("group", { name: "Olena Hrytsenko" }).locator("video")).toBeAttached({ timeout: 15_000 });
  await expect.poll(() => playing(marek, '[role="group"][aria-label="Olena Hrytsenko"] video'), { timeout: 15_000 }).toBe(true);
  await expect.poll(() => playing(page, '[role="group"][aria-label="Marek Nowak"] video'), { timeout: 15_000 }).toBe(true);

  // Olena mutes and raises her hand: Marek sees it.
  await olena.getByRole("button", { name: en.mute }).click();
  await olena.getByRole("button", { name: en.raiseHand }).click();
  const hers = his.getByRole("group", { name: "Olena Hrytsenko" });
  await expect(hers.getByText(en.muted)).toBeVisible({ timeout: 15_000 });
  await expect(hers.getByRole("img", { name: en.handRaised })).toBeVisible();

  // Olena shares her screen: Marek sees it on the stage.
  await olena.getByRole("button", { name: en.shareScreen }).click();
  await expect(his.getByText(en.presenting.replace("{who}", "Olena Hrytsenko"))).toBeVisible({ timeout: 15_000 });
  await expect.poll(() => playing(marek, "video.screen"), { timeout: 15_000 }).toBe(true);

  // Marek leaves: back in the channel; Olena is alone.
  await his.getByRole("button", { name: en.leave }).click();
  await expect(marek).toHaveURL(/\/chat\?room=/);
  await expect(olena.getByText(en.inCall.one.replace("{count}", "1"))).toBeVisible({ timeout: 15_000 });
  expect(errors).toEqual([]);
});
