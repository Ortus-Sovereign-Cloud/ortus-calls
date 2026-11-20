import type { CallsKey, CallsMessage } from "./en.js";

const messages: Record<CallsKey, CallsMessage> = {
  calls: "Appels",
  call: "{room} · appel",
  backToChannel: "Retour au canal",
  joinCall: "Rejoindre l'appel",
  startCall: "Démarrer un appel",
  inCall: {"one": "{count} personne dans l'appel", "other": "{count} personnes dans l'appel"},
  mute: "Couper le micro",
  unmute: "Réactiver le micro",
  camera: "Caméra",
  cameraOn: "Activer la caméra",
  shareScreen: "Partager l'écran",
  stopSharing: "Arrêter le partage",
  raiseHand: "Lever la main",
  lowerHand: "Baisser la main",
  leave: "Quitter",
  you: "Vous",
  muted: "Micro coupé",
  handRaised: "Main levée",
  presenting: "{who} présente",
  nothingShown: "Rien n'est encore montré",
  nothingShownHint: "Partagez votre écran pour montrer quelque chose à tous.",
  participants: "Participants",
  noDevices: "Ni caméra ni micro autorisés : vous êtes dans l'appel pour écouter et regarder.",
  joining: "Connexion…",
  callEnded: "L'appel est terminé.",
  notFound: "Ce salon n'existe pas.",
  reconnecting: "Connexion perdue, nouvelle tentative…",
};

export default messages;
