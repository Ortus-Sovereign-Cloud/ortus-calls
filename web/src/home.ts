// The Calls page without a room (/calls): the calls going on in one's
// rooms, to join, and one's channels and conversations, to start a call in.
import { LitElement, LocalizeController, call, css, customElement, html, nothing, state } from "ortus-kit";
import { t } from "./i18n.js";

export interface Live {
  room: string;
  name: string;
  kind: "channel" | "private" | "direct";
  startedAt: number;
  people: { id: string; name: string }[];
}

interface Room {
  id: string;
  kind: "channel" | "private" | "direct";
  name: string;
  state: string;
  encrypted: boolean;
  with?: string[];
}

/** A room's name as people see it: a conversation by its people. */
export function roomName(kind: string, name: string, people: string[]): string {
  return kind === "direct" ? people.join(", ") || name : `# ${name}`;
}

@customElement("ortus-calls-home")
export class OrtusCallsHome extends LitElement {
  static override styles = css`
    :host {
      display: block;
      padding: 32px;
      max-inline-size: 920px;
      color: var(--ortus-text);
    }
    h1 {
      margin: 0 0 24px;
      font-size: 24px;
    }
    h2 {
      margin: 28px 0 12px;
      color: var(--ortus-text-2);
      font-size: 14px;
      font-weight: 600;
    }
    ul {
      display: grid;
      gap: 10px;
      margin: 0;
      padding: 0;
      list-style: none;
    }
    li {
      display: flex;
      align-items: center;
      gap: 14px;
      padding: 14px 16px;
      border-radius: 14px;
      border: 1px solid var(--ortus-line);
      background: var(--ortus-surface);
    }
    .live {
      border-color: color-mix(in srgb, var(--ortus-teal) 50%, var(--ortus-line));
    }
    .what {
      flex: 1;
      display: flex;
      flex-direction: column;
      min-inline-size: 0;
    }
    .what strong {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .what small {
      color: var(--ortus-text-2);
    }
    .faces {
      display: flex;
    }
    .faces ortus-avatar {
      margin-inline-start: -8px;
      border: 2px solid var(--ortus-surface);
    }
    a.button {
      flex: none;
      padding: 8px 14px;
      border-radius: 10px;
      border: 1px solid var(--ortus-line-strong);
      color: inherit;
      font-size: 14px;
      font-weight: 600;
      text-decoration: none;
    }
    a.button.primary {
      border-color: transparent;
      background: var(--ortus-teal);
      color: var(--ortus-bg);
    }
    .quiet {
      color: var(--ortus-text-2);
    }
  `;

  private readonly localize = new LocalizeController(this);
  @state() private live: Live[] = [];
  @state() private rooms: Room[] = [];
  @state() private people: Record<string, { name: string }> = {};
  @state() private loaded = false;

  override connectedCallback(): void {
    super.connectedCallback();
    void Promise.all([
      call<{ live: Live[] }>("GET", "/calls/live").then((r) => (this.live = r.live)).catch(() => undefined),
      // The rooms to start a call in come from chat (a call is a room's).
      call<{ rooms: Room[]; people: Record<string, { name: string }> }>("GET", "/chat/rooms")
        .then((r) => {
          this.rooms = r.rooms.filter((x) => x.state !== "invite" && !x.encrypted);
          this.people = r.people;
        })
        .catch(() => undefined),
    ]).finally(() => (this.loaded = true));
  }

  private since(ms: number): string {
    const minutes = Math.max(1, Math.round((Date.now() - ms) / 60_000));
    return new Intl.RelativeTimeFormat(this.localize.lang, { numeric: "auto" }).format(-minutes, "minute");
  }

  override render() {
    if (!this.loaded) return nothing;
    const going = new Set(this.live.map((l) => l.room));
    return html`
      <h1>${t(this.localize, "calls")}</h1>
      <section aria-labelledby="live">
        <h2 id="live">${t(this.localize, "happeningNow")}</h2>
        ${this.live.length
          ? html`<ul>
              ${this.live.map(
                (l) => html`<li class="live">
                  <span class="what">
                    <strong dir="auto">${roomName(l.kind, l.name, l.people.map((p) => p.name))}</strong>
                    <small>${t(this.localize, "inCall", { count: l.people.length })} · ${t(this.localize, "startedAgo", { when: this.since(l.startedAt) })}</small>
                  </span>
                  <span class="faces" aria-hidden="true">
                    ${l.people.slice(0, 4).map((p) => html`<ortus-avatar size="28" radius="14" .name=${p.name} .user=${p.id}></ortus-avatar>`)}
                  </span>
                  <a class="button primary" href=${`/calls?room=${encodeURIComponent(l.room)}`}>${t(this.localize, "joinCall")}</a>
                </li>`,
              )}
            </ul>`
          : html`<p class="quiet">${t(this.localize, "noCallsNow")}</p>`}
      </section>
      <section aria-labelledby="start">
        <h2 id="start">${t(this.localize, "startCall")}</h2>
        ${this.rooms.filter((r) => !going.has(r.id)).length
          ? html`<ul>
              ${this.rooms
                .filter((r) => !going.has(r.id))
                .slice(0, 30)
                .map(
                  (r) => html`<li>
                    <span class="what"><strong dir="auto">${roomName(r.kind, r.name, (r.with ?? []).map((id) => this.people[id]?.name ?? id))}</strong></span>
                    <a class="button" href=${`/calls?room=${encodeURIComponent(r.id)}`}>${t(this.localize, "startHere")}</a>
                  </li>`,
                )}
            </ul>`
          : html`<p class="quiet">${t(this.localize, "noRooms")}</p>`}
      </section>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-calls-home": OrtusCallsHome;
  }
}
