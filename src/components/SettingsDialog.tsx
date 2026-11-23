import type { ComponentProps } from "react";
import { Modal } from "./Modal";
import { Settings } from "./Settings";
import { playSound } from "../lib/sounds";
import type { AlertPrefs } from "../lib/attention";

type SettingsProps = ComponentProps<typeof Settings>;

/**
 * Settings in its modal. A sound is heard as its alert is turned on, so the
 * user knows what to listen for; keeping the pick is the caller's.
 */
export function SettingsDialog({
  open,
  onClose,
  alerts,
  onAlerts,
  ...settings
}: Omit<SettingsProps, "alerts" | "onAlerts"> & {
  open: boolean;
  onClose: () => void;
  alerts: AlertPrefs;
  onAlerts: (next: AlertPrefs) => void;
}) {
  return (
    <Modal title="Settings" wide open={open} onClose={onClose}>
      <Settings
        {...settings}
        alerts={alerts}
        onAlerts={(next) => {
          // A sound is heard as it is turned on, so the user knows what to listen for.
          const turnedOn = (["done", "failed"] as const).find((kind) => next.sound[kind] && !alerts.sound[kind]);
          if (turnedOn) playSound(turnedOn === "done" ? "done" : "attention").catch(() => {});
          onAlerts(next);
        }}
      />
    </Modal>
  );
}