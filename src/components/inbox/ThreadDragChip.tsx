import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { useThreadDrag } from "../../lib/threadDrag";
import { Icon } from "../ui/Icon";

/** Follows the pointer while mail is dragged: what is moving, and where it
 *  would land ("Move to Receipts", "Add to Starred" for a view tab) or that
 *  the tab under it refuses mail. */
export function ThreadDragChip() {
  const { t } = useTranslation();
  const drag = useThreadDrag((s) => s.drag);
  if (!drag) return null;
  const { over } = drag;
  const what =
    drag.ids.length > 1
      ? t("inbox:drag.count", { count: drag.ids.length })
      : drag.subject || t("inbox:pane.noSubject");
  return createPortal(
    <div
      aria-hidden
      className="pointer-events-none fixed z-[100] flex max-w-[320px] items-center gap-2 rounded-control border border-hairline bg-bg1 px-2.5 py-1.5 text-sm shadow-elev-2"
      style={{ left: drag.x + 14, top: drag.y + 12 }}
    >
      <Icon name="mail" size={13} className="shrink-0 text-ink-faint" />
      <span className="min-w-0 truncate text-ink">{what}</span>
      {over && (
        <span className={`shrink-0 truncate ${over.ok ? "font-medium text-accent" : "text-ink-faint"}`}>
          {over.ok
            ? t(over.view ? "inbox:drag.addTo" : "inbox:drag.moveTo", { name: over.name })
            : t("inbox:drag.cantDrop", { name: over.name })}
        </span>
      )}
    </div>,
    document.body,
  );
}
