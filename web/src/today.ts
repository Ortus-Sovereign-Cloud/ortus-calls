// The calls card on the start page (ortus-calls-today, from
// /assets/apps/calls/today.js): the calls going on in one's rooms, to join.
import { LitElement, LocalizeController, call, css, customElement, html, nothing, state } from "ortus-kit";
import { roomName, type Live } from "./home.js";
import { ready, t } from "./i18n.js";

await ready;

@customElement("ortus-calls-today")
export class OrtusCallsToday extends LitElement {
  static override styles = css`
    :host {
      display: block;
    }
    section {
      padding: 20px;
      border-radius: 18px;
      border: 1px solid var(--ortus-line);
      background: var(--ortus-surface);
    }
    h2 {
      margin: 0 0 12px;
      font-size: 17px;
      font-weight: 600;
    }
    ul {
      margin: 0;
      padding: 0;
      list-style: none;
    }
    li {
      display: flex;
      align-items: center;
      gap: 10px;
      padding-block: 6px;
    }
    .what {
      flex: 1;
      display: flex;
      flex-direction: column;
      min-inline-size: 0;
    }
    .what strong {
      font-size: 14px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .what small {
      color: var(--ortus-text-2);
      font-size: 13px;
    }
    .join {
      padding: 4px 12px;
      border-radius: 999px;
      background: var(--ortus-teal);
      color: var(--ortus-bg);
      font-size: 13px;
      font-weight: 600;
      text-decoration: none;
    }
    .quiet {
      margin: 0;
      color: var(--ortus-text-2);
      font-size: 14px;
    }
    footer {
      margin-block-start: 12px;
      font-size: 14px;
    }
    footer a {
      color: var(--ortus-accent);
    }
  `;

  private readonly localize = new LocalizeController(this);
  @state() private live: Live[] = [];
  @state() private loaded = false;

  override connectedCallback(): void {
    super.connectedCallback();
    void call<{ live: Live[] }>("GET", "/calls/live")
      .then((r) => (this.live = r.live))
      .catch(() => undefined)
      .finally(() => (this.loaded = true));
  }

  override render() {
    if (!this.loaded) return nothing;
    return html`
      <section aria-labelledby="calls-today">
        <h2 id="calls-today">${t(this.localize, "happeningNow")}</h2>
        ${this.live.length
          ? html`<ul>
              ${this.live.slice(0, 4).map(
                (l) => html`<li>
                  <span class="what">
                    <strong dir="auto">${roomName(l.kind, l.name, l.people.map((p) => p.name))}</strong>
                    <small>${t(this.localize, "inCall", { count: l.people.length })}</small>
                  </span>
                  <a class="join" href=${`/calls?room=${encodeURIComponent(l.room)}`}>${t(this.localize, "join")}</a>
                </li>`,
              )}
            </ul>`
          : html`<p class="quiet">${t(this.localize, "noCallsNow")}</p>`}
        <footer><a href="/calls">${t(this.localize, "startCall")}</a></footer>
      </section>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-calls-today": OrtusCallsToday;
  }
}
