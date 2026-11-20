import type { CallsKey, CallsMessage } from "./en.js";

const messages: Record<CallsKey, CallsMessage> = {
  calls: "المكالمات",
  call: "{room} · مكالمة",
  backToChannel: "العودة إلى القناة",
  joinCall: "الانضمام إلى المكالمة",
  startCall: "بدء مكالمة",
  inCall: {"zero": "لا أحد في المكالمة", "one": "شخص واحد في المكالمة", "two": "شخصان في المكالمة", "few": "{count} أشخاص في المكالمة", "many": "{count} شخصًا في المكالمة", "other": "{count} شخص في المكالمة"},
  mute: "كتم الصوت",
  unmute: "إلغاء الكتم",
  camera: "الكاميرا",
  cameraOn: "تشغيل الكاميرا",
  shareScreen: "مشاركة الشاشة",
  stopSharing: "إيقاف المشاركة",
  raiseHand: "رفع اليد",
  lowerHand: "خفض اليد",
  leave: "مغادرة",
  you: "أنت",
  muted: "مكتوم",
  handRaised: "اليد مرفوعة",
  presenting: "{who} يعرض",
  nothingShown: "لا يُعرض شيء بعد",
  nothingShownHint: "شارك شاشتك لتعرض شيئًا على الجميع.",
  participants: "المشاركون",
  noDevices: "الكاميرا والميكروفون غير مسموح بهما: أنت في المكالمة للاستماع والمشاهدة.",
  joining: "جارٍ الانضمام…",
  callEnded: "انتهت المكالمة.",
  notFound: "لا توجد غرفة كهذه.",
  reconnecting: "انقطع الاتصال، جارٍ الانضمام مجددًا…",
};

export default messages;
