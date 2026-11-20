// The calls client: the element the shell shows at /calls (a room's call:
// /calls?room=<chat room id>).
import { LitElement, css, customElement, html } from "ortus-kit";
import "./call.js";
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
    return html`<ortus-call></ortus-call>`;
  }
}
