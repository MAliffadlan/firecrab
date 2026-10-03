import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

interface StatusLabelProps {
  children: string;
  className: string;
  accessibleLabel: string;
  tooltip: ReactNode;
  state?: string;
  outcome?: string;
}

export default function StatusLabel({ children, className, accessibleLabel, tooltip, state, outcome }: StatusLabelProps) {
  const id = useId();
  const trigger = useRef<HTMLElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const hovered = useRef(false);
  const focused = useRef(false);
  const closeTimer = useRef<number | null>(null);
  const [open, setOpen] = useState(false);

  function cancelClose() {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  }

  function show() {
    cancelClose();
    setOpen(true);
  }

  function leave() {
    hovered.current = false;
    cancelClose();
    closeTimer.current = window.setTimeout(() => {
      if (!hovered.current && !focused.current) setOpen(false);
    }, 120);
  }

  useLayoutEffect(() => {
    if (!open || !trigger.current || !panel.current) return;
    function position() {
      if (!trigger.current || !panel.current) return;
      const anchor = trigger.current.getBoundingClientRect();
      const box = panel.current.getBoundingClientRect();
      const margin = 12;
      const gap = 8;
      const below = window.innerHeight - margin - anchor.bottom - gap;
      const above = anchor.top - gap - margin;
      const left = Math.max(margin, Math.min(anchor.left, window.innerWidth - box.width - margin));
      const top = below >= box.height || below >= above
        ? Math.max(margin, Math.min(anchor.bottom + gap, window.innerHeight - box.height - margin))
        : Math.max(margin, anchor.top - gap - box.height);
      panel.current.style.left = `${left}px`;
      panel.current.style.top = `${top}px`;
      panel.current.style.visibility = "visible";
    }
    position();
    window.addEventListener("resize", position);
    window.addEventListener("scroll", position, true);
    return () => {
      window.removeEventListener("resize", position);
      window.removeEventListener("scroll", position, true);
    };
  }, [open, tooltip]);

  useEffect(() => {
    if (!open) return;
    function dismiss() { setOpen(false); }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") dismiss();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      cancelClose();
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <>
      <strong
        ref={trigger}
        className={`state-label ${className}`}
        role="status"
        aria-label={accessibleLabel}
        aria-describedby={open ? id : undefined}
        data-state={state}
        data-outcome={outcome}
        tabIndex={0}
        onMouseEnter={() => { hovered.current = true; show(); }}
        onMouseLeave={leave}
        onFocus={() => { focused.current = true; show(); }}
        onBlur={() => { focused.current = false; leave(); }}
      >
        {children}
      </strong>
      {open && createPortal(
        <div
          ref={panel}
          id={id}
          role="tooltip"
          className="status-tooltip"
          style={{ visibility: "hidden" }}
          onMouseEnter={() => { hovered.current = true; cancelClose(); }}
          onMouseLeave={leave}
        >
          {tooltip}
        </div>,
        document.body,
      )}
    </>
  );
}
