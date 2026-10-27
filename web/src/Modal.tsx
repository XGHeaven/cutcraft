import { useEffect, useId, useRef, type ReactNode } from "react";

import { Icon } from "./Icons";

export function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const id = useId();
  useEffect(() => {
    const dialog = ref.current!;
    const previous = document.activeElement as HTMLElement | null;
    dialog.showModal();
    return () => {
      dialog.close();
      previous?.focus();
    };
  }, []);
  return (
    <dialog
      ref={ref}
      className="editor-dialog"
      aria-labelledby={id}
      onCancel={onClose}
    >
      <header>
        <h2 id={id}>{title}</h2>
        <button aria-label="Close dialog" onClick={onClose}>
          <Icon name="close" />
        </button>
      </header>
      <div className="dialog-body">{children}</div>
    </dialog>
  );
}
