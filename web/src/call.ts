// The call screen (WebCall board): a room's call with tiles for everyone
// in it (video, or initials when the camera is off), who speaks, a raised
// hand, mute; the shared screen in the main area; controls to mute, turn
// the camera off, share the screen, raise a hand and leave.
//
// Media: one RTCPeerConnection to the call's media node. The first offer
// carries three sending transceivers (microphone, camera, screen), so
// sharing the screen later is a track swap, never a new negotiation; the
// media node offers each other person's tracks over a data channel, and
// this side answers. Remote tracks arrive with the sender's session as
// their stream id and the sender's own mid as their track id.
import { ApiError, LitElement, LocalizeController, call, css, customElement, html, nothing, state, type TemplateResult } from "ortus-kit";
import { t } from "./i18n.js";

interface SessionInfo {
  session: string;
  user: string;
  joinedAt: number;
  muted: boolean;
  camera: boolean;
  screen: string | null;
  hand: number | null;
}

interface CallInfo {
  id: string;
  room: string;
  startedAt: number;
  sessions: SessionInfo[];
  people: Record<string, { name: string }>;
}

interface Remote {
  stream: MediaStream;
  /** By the sender's own mid. */
  tracks: Map<string, MediaStreamTrack>;
}

const TONES = ["#A9B6FF", "#F5B57A", "#6FE0D5", "#C6F06B", "#FF9A92", "#DDE3F8"];

function tone(text: string): string {
  let h = 0;
  for (const c of text) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return TONES[h % TONES.length]!;
}

function initials(name: string): string {
  return (
    name
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((p) => p[0]!.toUpperCase())
      .join("") || "?"
  );
}

@customElement("ortus-call")
export class WebCall extends LitElement {
  static override styles = css`
    :host {
      display: flex;
      flex-direction: column;
      min-block-size: 100vh;
      background: var(--ortus-bg, #07080c);
      color: var(--ortus-text, #eceef3);
      font-size: 15px;
    }
    header {
      display: flex;
      align-items: center;
      gap: 14px;
      padding-block: 14px;
      padding-inline: 24px;
      flex-wrap: wrap;
    }
    header a {
      inline-size: 40px;
      block-size: 40px;
      border-radius: 12px;
      display: flex;
      align-items: center;
      justify-content: center;
      color: inherit;
    }
    header a:dir(rtl) svg {
      transform: scaleX(-1);
    }
    header strong {
      font-weight: 600;
    }
    .count {
      font-size: 13px;
      color: var(--ortus-text-3, #9198a8);
    }
    .body {
      flex: 1;
      display: flex;
      flex-wrap: wrap;
      gap: 16px;
      padding-block: 0 16px;
      padding-inline: 24px;
    }
    section.stage {
      flex: 999 1 560px;
      min-inline-size: 0;
      border-radius: 20px;
      background: #11131a;
      border: 1px solid #1e2129;
      overflow: hidden;
      display: flex;
      flex-direction: column;
    }
    .stage-head {
      display: flex;
      align-items: center;
      gap: 12px;
      padding-block: 14px;
      padding-inline: 20px;
      border-block-end: 1px solid #1e2129;
      font-weight: 500;
    }
    .stage-body {
      flex: 1;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 20px;
      min-block-size: 360px;
    }
    .stage-body video {
      max-inline-size: 100%;
      max-block-size: 100%;
      border-radius: 12px;
      background: #000;
    }
    .empty {
      text-align: center;
      color: var(--ortus-text-3, #9198a8);
    }
    .empty strong {
      display: block;
      color: var(--ortus-text, #eceef3);
      font-weight: 500;
      margin-block-end: 6px;
    }
    aside {
      flex: 1 1 300px;
      max-inline-size: 360px;
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
    .tiles {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 12px;
    }
    .tile {
      position: relative;
      block-size: 112px;
      border-radius: 16px;
      background: #13151c;
      border: 1px solid #1e2129;
      display: flex;
      align-items: center;
      justify-content: center;
      overflow: hidden;
    }
    .tile.speaking {
      border-color: transparent;
      box-shadow: 0 0 0 2px var(--ortus-accent, #8ea2ff);
    }
    .tile video {
      position: absolute;
      inset: 0;
      inline-size: 100%;
      block-size: 100%;
      object-fit: cover;
    }
    .avatar {
      inline-size: 48px;
      block-size: 48px;
      margin-block-end: 14px;
      border-radius: 16px;
      color: #0a0b10;
      font-weight: 700;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    .name,
    .state {
      position: absolute;
      inset-block-end: 10px;
      font-size: 13px;
      text-shadow: 0 1px 2px #000;
    }
    .name {
      inset-inline-start: 12px;
      max-inline-size: calc(100% - 84px);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-weight: 500;
    }
    .state {
      inset-inline-end: 12px;
      color: var(--ortus-text-3, #9198a8);
    }
    .hand {
      position: absolute;
      inset-block-start: 8px;
      inset-inline-end: 10px;
      inline-size: 26px;
      block-size: 26px;
      border-radius: 8px;
      background: #f5b57a;
      color: #0a0b10;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    .controls {
      display: flex;
      justify-content: center;
      padding-block: 0 24px;
      padding-inline: 24px;
    }
    .bar {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      padding: 8px;
      border-radius: 20px;
      background: #171a23;
      border: 1px solid #2a2e3a;
    }
    .bar button,
    .join button {
      block-size: 44px;
      padding-inline: 20px;
      border-radius: 14px;
      border: 0;
      background: #1e2230;
      color: inherit;
      font: 500 14px inherit;
      cursor: pointer;
    }
    .bar button[aria-pressed="true"] {
      background: #2b3146;
    }
    .bar button.leave,
    .join button {
      background: #ff7a6e;
      color: #0a0b10;
      font-weight: 600;
      padding-inline: 22px;
    }
    .join {
      flex: 1;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: 16px;
    }
    .join button {
      background: var(--ortus-accent, #8ea2ff);
    }
    .note {
      color: var(--ortus-text-3, #9198a8);
      font-size: 14px;
    }
    .error {
      color: #ff9a92;
    }
    audio {
      display: none;
    }
  `;

  private localize = new LocalizeController(this);
  @state() private room = "";
  @state() private name = "";
  @state() private info: CallInfo | null = null;
  @state() private session = "";
  @state() private joining = false;
  @state() private error = "";
  @state() private muted = false;
  @state() private camera = true;
  @state() private sharing = false;
  @state() private hand = false;
  @state() private noDevices = false;
  @state() private speaking = new Set<string>();
  @state() private version = 0;
  private pc: RTCPeerConnection | null = null;
  private channel: RTCDataChannel | null = null;
  private local: MediaStream | null = null;
  private screenStream: MediaStream | null = null;
  private senders: { audio?: RTCRtpSender; camera?: RTCRtpSender; screen?: RTCRtpTransceiver } = {};
  private remotes = new Map<string, Remote>();
  private timers: number[] = [];
  private audio: AudioContext | null = null;
  private meters = new Map<string, AnalyserNode>();

  override connectedCallback(): void {
    super.connectedCallback();
    this.room = new URLSearchParams(location.search).get("room") ?? "";
    void this.load();
    this.timers.push(window.setInterval(() => void this.load(), 3000));
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    for (const t of this.timers) clearInterval(t);
    this.timers = [];
    this.hangUp();
  }

  private async load(): Promise<void> {
    if (!this.room) return;
    try {
      const r = await call<{ call: CallInfo | null; name: string }>("GET", `/calls/rooms/${encodeURIComponent(this.room)}`);
      this.name = r.name;
      this.info = r.call;
      if (this.session && (!r.call || !r.call.sessions.some((s) => s.session === this.session))) {
        // Our part ended (the media node went, or we were away too long).
        this.hangUp();
      }
    } catch (e) {
      this.error = e instanceof ApiError && e.status === 404 ? t(this.localize, "notFound") : String(e);
    }
  }

  private async join(): Promise<void> {
    this.joining = true;
    this.error = "";
    try {
      try {
        this.local = await navigator.mediaDevices.getUserMedia({ audio: true, video: { width: 640, height: 360 } });
      } catch {
        try {
          this.local = await navigator.mediaDevices.getUserMedia({ audio: true });
          this.camera = false;
        } catch {
          this.local = null;
          this.noDevices = true;
        }
      }
      const r = await call<{ call: CallInfo; session: string }>("POST", `/calls/rooms/${encodeURIComponent(this.room)}/join`);
      this.session = r.session;
      this.info = r.call;
      await this.connect();
      this.watch("me", this.local);
      await this.report();
      this.timers.push(window.setInterval(() => void this.report(), 10_000));
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
      this.hangUp();
    } finally {
      this.joining = false;
    }
  }

  private async connect(): Promise<void> {
    const pc = new RTCPeerConnection();
    this.pc = pc;
    const channel = pc.createDataChannel("signalling");
    this.channel = channel;
    channel.onmessage = (e) => void this.signal(String(e.data));
    const audio = this.local?.getAudioTracks()[0] ?? null;
    const video = this.local?.getVideoTracks()[0] ?? null;
    this.senders.audio = pc.addTransceiver(audio ?? "audio", { direction: "sendonly", streams: this.local ? [this.local] : [] }).sender;
    this.senders.camera = pc.addTransceiver(video ?? "video", { direction: "sendonly", streams: this.local ? [this.local] : [] }).sender;
    this.senders.screen = pc.addTransceiver("video", { direction: "sendonly" });
    pc.ontrack = (e) => this.track(e);
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "failed") this.error = t(this.localize, "reconnecting");
    };
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    const r = await call<{ answer: RTCSessionDescriptionInit }>("POST", `/calls/sessions/${encodeURIComponent(this.session)}/offer`, { offer: { type: "offer", sdp: offer.sdp } });
    await pc.setRemoteDescription(r.answer);
  }

  /** The media node offers others' tracks (or stops them); we answer. */
  private async signal(data: string): Promise<void> {
    const pc = this.pc;
    if (!pc) return;
    const msg = JSON.parse(data) as RTCSessionDescriptionInit;
    if (msg.type !== "offer") return;
    await pc.setRemoteDescription(msg);
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    this.channel?.send(JSON.stringify({ type: "answer", sdp: answer.sdp }));
  }

  private track(e: RTCTrackEvent): void {
    const session = e.streams[0]?.id;
    if (!session) return;
    const remote = this.remotes.get(session) ?? { stream: new MediaStream(), tracks: new Map() };
    remote.tracks.set(e.track.id, e.track);
    remote.stream.addTrack(e.track);
    this.remotes.set(session, remote);
    if (e.track.kind === "audio") this.watch(session, new MediaStream([e.track]));
    e.track.onended = () => {
      remote.tracks.delete(e.track.id);
      this.version++;
    };
    this.version++;
  }

  /** Who speaks: the loudness of each person's audio. */
  private watch(who: string, stream: MediaStream | null): void {
    const track = stream?.getAudioTracks()[0];
    if (!track) return;
    this.audio ??= new AudioContext();
    const analyser = this.audio.createAnalyser();
    analyser.fftSize = 512;
    this.audio.createMediaStreamSource(new MediaStream([track])).connect(analyser);
    this.meters.set(who, analyser);
    if (this.meters.size === 1) {
      this.timers.push(
        window.setInterval(() => {
          const now = new Set<string>();
          const buf = new Uint8Array(512);
          for (const [k, a] of this.meters) {
            a.getByteTimeDomainData(buf);
            let sum = 0;
            for (const v of buf) sum += (v - 128) * (v - 128);
            if (Math.sqrt(sum / buf.length) > 6) now.add(k);
          }
          if ([...now].join() !== [...this.speaking].join()) this.speaking = now;
        }, 250),
      );
    }
  }

  /** Says what this side shows (and that it is still here). */
  private async report(): Promise<void> {
    if (!this.session) return;
    const screen = this.sharing ? (this.senders.screen?.mid ?? null) : null;
    const r = await call<{ call: CallInfo }>("POST", `/calls/sessions/${encodeURIComponent(this.session)}/state`, { muted: this.muted, camera: this.camera, hand: this.hand, screen }).catch(() => null);
    if (r) this.info = r.call;
  }

  private toggleMute(): void {
    this.muted = !this.muted;
    for (const t of this.local?.getAudioTracks() ?? []) t.enabled = !this.muted;
    void this.report();
  }

  private toggleCamera(): void {
    this.camera = !this.camera;
    for (const t of this.local?.getVideoTracks() ?? []) t.enabled = this.camera;
    void this.report();
  }

  private async toggleScreen(): Promise<void> {
    if (this.sharing) {
      await this.stopScreen();
      return;
    }
    try {
      this.screenStream = await navigator.mediaDevices.getDisplayMedia({ video: true });
    } catch {
      return;
    }
    const track = this.screenStream.getVideoTracks()[0]!;
    track.onended = () => void this.stopScreen();
    await this.senders.screen?.sender.replaceTrack(track);
    this.sharing = true;
    await this.report();
  }

  private async stopScreen(): Promise<void> {
    for (const t of this.screenStream?.getTracks() ?? []) t.stop();
    this.screenStream = null;
    await this.senders.screen?.sender.replaceTrack(null);
    this.sharing = false;
    await this.report();
  }

  private toggleHand(): void {
    this.hand = !this.hand;
    void this.report();
  }

  private async leave(): Promise<void> {
    if (this.session) await call("POST", `/calls/sessions/${encodeURIComponent(this.session)}/leave`).catch(() => undefined);
    this.hangUp();
    location.assign(`/chat?room=${encodeURIComponent(this.room)}`);
  }

  private hangUp(): void {
    for (const t of [...(this.local?.getTracks() ?? []), ...(this.screenStream?.getTracks() ?? [])]) t.stop();
    this.pc?.close();
    this.pc = null;
    this.channel = null;
    this.session = "";
    this.local = null;
    this.screenStream = null;
    this.sharing = false;
    this.remotes.clear();
    this.meters.clear();
    void this.audio?.close();
    this.audio = null;
  }

  private nameOf(s: SessionInfo): string {
    if (s.session === this.session) return t(this.localize, "you");
    return this.info?.people[s.user]?.name ?? "";
  }

  private tile(s: SessionInfo): TemplateResult {
    const mine = s.session === this.session;
    const remote = this.remotes.get(s.session);
    const name = this.nameOf(s);
    // The camera's track: the video track that is not the screen.
    const cameraTrack = mine ? this.local?.getVideoTracks()[0] : [...(remote?.tracks.entries() ?? [])].find(([mid, t]) => t.kind === "video" && mid !== s.screen)?.[1];
    const video = s.camera && cameraTrack ? new MediaStream([cameraTrack]) : null;
    const speaking = this.speaking.has(mine ? "me" : s.session) && !s.muted;
    const full = mine ? name : this.info?.people[s.user]?.name ?? "";
    return html`<div class="tile ${speaking ? "speaking" : ""}" role="group" aria-label=${full}>
      ${video ? html`<video autoplay playsinline muted .srcObject=${video}></video>` : html`<span class="avatar" aria-hidden="true" .style=${`background: ${tone(full)}`}>${initials(full)}</span>`}
      <span class="name">${name}</span>
      ${s.muted ? html`<span class="state">${t(this.localize, "muted")}</span>` : nothing}
      ${s.hand
        ? html`<span class="hand" role="img" aria-label=${t(this.localize, "handRaised")}
            ><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 11V6a2 2 0 0 0-4 0v5M14 10V4a2 2 0 0 0-4 0v6M10 10.5V6a2 2 0 0 0-4 0v8" /><path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15" /></svg
          ></span>`
        : nothing}
    </div>`;
  }

  private stage(): TemplateResult {
    const presenter = this.info?.sessions.find((s) => s.screen);
    if (presenter) {
      const mine = presenter.session === this.session;
      const track = mine ? this.screenStream?.getVideoTracks()[0] : this.remotes.get(presenter.session)?.tracks.get(presenter.screen!);
      return html`<div class="stage-head">${t(this.localize, "presenting", { who: mine ? t(this.localize, "you") : this.nameOf(presenter) })}</div>
        <div class="stage-body">${track ? html`<video class="screen" autoplay playsinline muted .srcObject=${new MediaStream([track])}></video>` : nothing}</div>`;
    }
    return html`<div class="stage-body"><p class="empty"><strong>${t(this.localize, "nothingShown")}</strong>${t(this.localize, "nothingShownHint")}</p></div>`;
  }

  override render(): TemplateResult {
    void this.version;
    const sessions = this.info?.sessions ?? [];
    const audios = [...this.remotes.entries()].flatMap(([session, r]) => [...r.tracks.values()].filter((t) => t.kind === "audio").map((t) => html`<audio autoplay .srcObject=${new MediaStream([t])} data-session=${session}></audio>`));
    return html`
      <header>
        <a href=${`/chat?room=${encodeURIComponent(this.room)}`} aria-label=${t(this.localize, "backToChannel")}><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M15 18l-6-6 6-6" /></svg></a>
        <strong>${t(this.localize, "call", { room: `# ${this.name}` })}</strong>
        ${sessions.length ? html`<span class="count">${t(this.localize, "inCall", { count: sessions.length })}</span>` : nothing}
      </header>
      ${this.session
        ? html`<div class="body">
              <section class="stage" aria-label=${t(this.localize, "shareScreen")}>${this.stage()}</section>
              <aside aria-label=${t(this.localize, "participants")}><div class="tiles">${sessions.map((s) => this.tile(s))}</div>
                ${this.noDevices ? html`<p class="note">${t(this.localize, "noDevices")}</p>` : nothing}
                ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : nothing}
              </aside>
            </div>
            <div class="controls">
              <div class="bar" role="toolbar">
                <button aria-pressed=${this.muted ? "true" : "false"} @click=${this.toggleMute}>${t(this.localize, this.muted ? "unmute" : "mute")}</button>
                <button aria-pressed=${this.camera ? "false" : "true"} @click=${this.toggleCamera}>${t(this.localize, this.camera ? "camera" : "cameraOn")}</button>
                <button aria-pressed=${this.sharing ? "true" : "false"} @click=${this.toggleScreen}>${t(this.localize, this.sharing ? "stopSharing" : "shareScreen")}</button>
                <button aria-pressed=${this.hand ? "true" : "false"} @click=${this.toggleHand}>${t(this.localize, this.hand ? "lowerHand" : "raiseHand")}</button>
                <button class="leave" @click=${this.leave}>${t(this.localize, "leave")}</button>
              </div>
            </div>
            ${audios}`
        : html`<div class="join">
            ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : nothing}
            ${this.room && !this.error.length
              ? html`<button ?disabled=${this.joining} @click=${this.join}>${this.joining ? t(this.localize, "joining") : t(this.localize, sessions.length ? "joinCall" : "startCall")}</button>`
              : nothing}
          </div>`}
    `;
  }
}
