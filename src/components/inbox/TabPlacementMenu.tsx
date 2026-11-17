import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { placementOf } from "../../lib/tabPlacement";
import { useSettings } from "../../queries/hooks";
import { setTabPlacement } from "../../queries/tabPlacement";
import { MenuItem, Popover } from "../ui/Menu";

/**
 * Right-click menu of a bundle row or a tab-bar tab in the bundled inbox:
 * move the tab to the top bar, or back into Important as a bundle. Takes
 * focus so Enter picks the item; Esc closes it.
 */
export function TabPlacementMenu({
  x,
  y,
  tabKey,
  onClose,
}: {
  x: number;
  y: number;
  tabKey: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const { data: settings } = useSettings();
  const top = placementOf(settings?.tabPlacement, tabKey) === "top";
  const itemRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    itemRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      // Enter / Space keep their native button activation; every key stays
      // out of the global shortcuts while the menu is open.
      e.stopPropagation();
      if (e.key === "Escape" || e.key === "Tab") {
        e.preventDefault();
        closeRef.current();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // Portaled to the body: the virtualized list and the scrolling tab bar
  // must not clip or offset the fixed menu.
  return createPortal(
    <Popover x={x} y={y} onClose={onClose}>
      <div ref={itemRef}>
        <MenuItem
          label={top ? t("inbox:placement.showAsBundle") : t("inbox:placement.moveToTop")}
          onClick={() => {
            onClose();
            void setTabPlacement(tabKey, top ? "bundle" : "top");
          }}
        />
      </div>
    </Popover>,
    document.body,
  );
}
