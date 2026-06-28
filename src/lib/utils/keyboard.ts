export function isNonComposingEnter(event: KeyboardEvent): boolean {
  // WebKit can report isComposing as false for the Enter key that confirms
  // IME composition, but still exposes it as the IME process key code.
  return event.key === "Enter" && !event.isComposing && event.keyCode !== 229;
}
