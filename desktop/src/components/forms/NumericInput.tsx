import React, { useEffect, useState } from "react";

interface BaseNumericInputProps {
  min?: number;
  max?: number;
  step?: number;
  placeholder?: string;
  className?: string;
  style?: React.CSSProperties;
  disabled?: boolean;
  "aria-label"?: string;
  title?: string;
}

interface RequiredNumericInputProps extends BaseNumericInputProps {
  optional?: false;
  value: number;
  onValueChange: (next: number) => void;
}

interface OptionalNumericInputProps extends BaseNumericInputProps {
  optional: true;
  value: number | undefined;
  onValueChange: (next: number | undefined) => void;
}

export type NumericInputProps =
  | RequiredNumericInputProps
  | OptionalNumericInputProps;

/**
 * Controlled integer text input with local draft buffering:
 * - Lets users freely clear/retype digits without snapping to 0/1 on Backspace.
 * - Never emits NaN, Infinity, or out-of-range intermediate numbers while typing.
 * - Reverts or clamps cleanly on blur so the document always stays valid.
 */
export const NumericInput: React.FC<NumericInputProps> = (props) => {
  const {
    min,
    max,
    step,
    placeholder,
    className,
    style,
    disabled,
    title,
    value,
  } = props;

  const formatVal = (v: number | undefined) =>
    v !== undefined && Number.isFinite(v) ? String(v) : "";

  const [draft, setDraft] = useState<string>(() => formatVal(value));
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!focused) {
      setDraft(formatVal(value));
    }
  }, [value, focused]);

  const isInRange = (n: number) => {
    if (min !== undefined && n < min) return false;
    if (max !== undefined && n > max) return false;
    return true;
  };

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const raw = e.target.value;
    setDraft(raw);
    const trimmed = raw.trim();
    if (trimmed === "") {
      if (props.optional) {
        props.onValueChange(undefined);
      }
      return;
    }
    if (!/^-?\d+$/.test(trimmed)) return;
    const parsed = Number(trimmed);
    if (!Number.isSafeInteger(parsed)) return;
    if (!isInRange(parsed)) return;
    if (props.optional) {
      props.onValueChange(parsed);
    } else {
      props.onValueChange(parsed);
    }
  };

  const handleBlur = () => {
    setFocused(false);
    const trimmed = draft.trim();
    if (trimmed === "") {
      if (props.optional) {
        props.onValueChange(undefined);
        setDraft("");
      } else {
        setDraft(formatVal(value));
      }
      return;
    }
    if (!/^-?\d+$/.test(trimmed)) {
      setDraft(formatVal(value));
      return;
    }
    let parsed = Number(trimmed);
    if (!Number.isSafeInteger(parsed)) {
      setDraft(formatVal(value));
      return;
    }
    if (min !== undefined && parsed < min) parsed = min;
    if (max !== undefined && parsed > max) parsed = max;
    if (step !== undefined && step > 1) {
      const snapped = Math.round(parsed / step) * step;
      if (
        (min === undefined || snapped >= min) &&
        (max === undefined || snapped <= max)
      ) {
        parsed = snapped;
      }
    }
    setDraft(String(parsed));
    if (props.optional) {
      props.onValueChange(parsed);
    } else {
      props.onValueChange(parsed);
    }
  };

  const trimmed = draft.trim();
  const showInvalid =
    focused &&
    trimmed !== "" &&
    (!/^-?\d+$/.test(trimmed) ||
      !Number.isSafeInteger(Number(trimmed)) ||
      !isInRange(Number(trimmed)));

  return (
    <input
      type="text"
      inputMode="numeric"
      className={className ? `mono ${className}` : "mono"}
      style={{
        ...style,
        ...(showInvalid
          ? { borderColor: "var(--status-warn-border)", background: "var(--status-warn-bg)" }
          : null),
      }}
      placeholder={placeholder}
      disabled={disabled}
      title={title}
      aria-label={props["aria-label"]}
      value={draft}
      onFocus={() => setFocused(true)}
      onChange={handleChange}
      onBlur={handleBlur}
    />
  );
};
