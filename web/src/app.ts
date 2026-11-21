// The calls client: the element the shell shows at /calls (a room's call:
// /calls?room=<chat room id>; without a room, the calls page).
import { LitElement, css, customElement, html } from "ortus-kit";
import "./call.js";
import "./home.js";
import { ready } from "./i18n.js";

await ready;

@customElement("ortus-calls-app")
export class OrtusCallsApp extends LitElement {
  static override styles = css`
    :host {
      display: block;
    }
  `;

  override render() {
    // A room's call, or, without a room, the calls page.
    return new URLSearchParams(location.search).get("room") ? html`<ortus-call></ortus-call>` : html`<ortus-calls-home></ortus-calls-home>`;
  }
}
