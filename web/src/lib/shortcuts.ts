const mac = /Mac|iPhone|iPad/.test(navigator.platform);

/** Label of the shortcut that opens and closes settings (⌘, or Ctrl+,). */
export const SETTINGS_SHORTCUT = mac ? "⌘," : "Ctrl+,";
