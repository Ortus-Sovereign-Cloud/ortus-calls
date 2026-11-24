// The call screen opens in the shell in every language, right to left in Arabic.
import { expect, test } from "@playwright/test";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import es from "../src/locales/es.js";
import fr from "../src/locales/fr.js";
import it from "../src/locales/it.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

for (const [code, texts, dir] of [["en", en, "ltr"], ["pl", pl, "ltr"], ["uk", uk, "ltr"], ["de", de, "ltr"], ["fr", fr, "ltr"], ["es", es, "ltr"], ["it", it, "ltr"], ["ar", ar, "rtl"]] as const) {
  test(`the call screen opens in ${code}`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
    await page.addInitScript((c) => localStorage.setItem("ortus.locale", c), code);
    await page.goto(`/calls?room=lang-${code}`);
    const screen = page.locator("ortus-call");
    await expect(screen.getByRole("button", { name: texts.startCall as string })).toBeVisible();
    await expect(screen.getByRole("link", { name: texts.backToChannel as string })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", dir);
    expect(errors).toEqual([]);
  });
}
