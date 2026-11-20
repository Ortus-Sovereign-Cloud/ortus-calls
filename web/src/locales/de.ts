import type { CallsKey, CallsMessage } from "./en.js";

const messages: Record<CallsKey, CallsMessage> = {
  calls: "Anrufe",
  call: "{room} · Anruf",
  backToChannel: "Zurück zum Kanal",
  joinCall: "Dem Anruf beitreten",
  startCall: "Anruf starten",
  inCall: {"one": "{count} Person im Anruf", "other": "{count} Personen im Anruf"},
  mute: "Stummschalten",
  unmute: "Stummschaltung aufheben",
  camera: "Kamera",
  cameraOn: "Kamera einschalten",
  shareScreen: "Bildschirm teilen",
  stopSharing: "Teilen beenden",
  raiseHand: "Hand heben",
  lowerHand: "Hand senken",
  leave: "Verlassen",
  you: "Du",
  muted: "Stumm",
  handRaised: "Hand gehoben",
  presenting: "{who} präsentiert",
  nothingShown: "Noch wird nichts gezeigt",
  nothingShownHint: "Teile deinen Bildschirm, um allen etwas zu zeigen.",
  participants: "Teilnehmende",
  noDevices: "Kamera und Mikrofon sind nicht erlaubt: Du bist im Anruf zum Zuhören und Zuschauen.",
  joining: "Beitreten…",
  callEnded: "Der Anruf ist beendet.",
  notFound: "Diesen Raum gibt es nicht.",
  reconnecting: "Verbindung verloren, trete erneut bei…",
};

export default messages;
