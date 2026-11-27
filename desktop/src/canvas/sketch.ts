/**
 * Style B1-Beta: Japanese Minimal Cute Rounded-Pen Line-Art (简笔画) Engine
 * with discrete 2-3 keyframe flipbook micro-animations (Flipnote / Baba Is You feel).
 */

export const INK = "#2c2522";
export const PAPER = "#fdfaf4";
export const PAPER_DOT = "#e2dacb";

export type DeviceMood = "idle" | "source" | "dest" | "happy" | "worried";

export function drawRoundedRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
) {
  const rad = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rad, y);
  ctx.lineTo(x + w - rad, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + rad);
  ctx.lineTo(x + w, y + h - rad);
  ctx.quadraticCurveTo(x + w, y + h, x + w - rad, y + h);
  ctx.lineTo(x + rad, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - rad);
  ctx.lineTo(x, y + rad);
  ctx.quadraticCurveTo(x, y, x + rad, y);
  ctx.closePath();
}

export function drawFlatPill(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  fill: string,
  stroke = INK,
  lineWidth = 1.75,
  radius?: number
) {
  const r = radius ?? h / 2;
  drawRoundedRectPath(ctx, x, y, w, h, r);
  ctx.fillStyle = fill;
  ctx.fill();
  ctx.strokeStyle = stroke;
  ctx.lineWidth = lineWidth;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.stroke();
}

function drawCuteFace(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  mood: DeviceMood,
  blink: boolean,
  step2: number
) {
  ctx.save();
  ctx.strokeStyle = INK;
  ctx.fillStyle = INK;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  const eyeDx = 11;

  if (mood === "worried") {
    ctx.lineWidth = 2.0;
    const eyeY = cy + (step2 === 1 ? 1.8 : 0.8);
    ctx.beginPath();
    ctx.arc(cx - eyeDx, eyeY, 3.8, Math.PI * 1.15, Math.PI * 1.85);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(cx + eyeDx, eyeY, 3.8, Math.PI * 1.15, Math.PI * 1.85);
    ctx.stroke();
    // 2-frame tiny worried mouth wobble
    if (step2 === 1) {
      ctx.beginPath();
      ctx.ellipse(cx, cy + 5.8, 2.5, 1.9, 0, 0, Math.PI * 2);
      ctx.stroke();
    } else {
      ctx.beginPath();
      ctx.arc(cx, cy + 6.5, 3.2, Math.PI * 1.18, Math.PI * 1.82);
      ctx.stroke();
    }
  } else if (mood === "happy") {
    // Rosy cheeks + 2-frame joyful laugh
    ctx.fillStyle = "rgba(243, 131, 112, 0.45)";
    ctx.beginPath();
    ctx.ellipse(cx - eyeDx - 5, cy + 4, 3.6, 2.1, 0, 0, Math.PI * 2);
    ctx.ellipse(cx + eyeDx + 5, cy + 4, 3.6, 2.1, 0, 0, Math.PI * 2);
    ctx.fill();

    ctx.lineWidth = 2.1;
    ctx.beginPath();
    ctx.arc(cx - eyeDx, cy + 1.5, 4.0, Math.PI * 1.1, Math.PI * 1.9);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(cx + eyeDx, cy + 1.5, 4.0, Math.PI * 1.1, Math.PI * 1.9);
    ctx.stroke();

    ctx.beginPath();
    ctx.arc(cx, cy + 2.2, step2 === 1 ? 5.0 : 4.2, 0.12 * Math.PI, 0.88 * Math.PI);
    ctx.stroke();
  } else if (mood === "dest") {
    if (blink) {
      ctx.lineWidth = 1.9;
      ctx.beginPath();
      ctx.moveTo(cx - eyeDx - 3, cy);
      ctx.lineTo(cx - eyeDx + 3, cy);
      ctx.moveTo(cx + eyeDx - 3, cy);
      ctx.lineTo(cx + eyeDx + 3, cy);
      ctx.stroke();
    } else {
      ctx.beginPath();
      ctx.arc(cx - eyeDx, cy, 2.4, 0, Math.PI * 2);
      ctx.arc(cx + eyeDx, cy, 2.4, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.beginPath();
    ctx.arc(cx, cy + 4.2, 1.4, 0, Math.PI * 2);
    ctx.fill();
  } else {
    if (blink) {
      ctx.lineWidth = 1.9;
      ctx.beginPath();
      ctx.moveTo(cx - eyeDx - 3, cy);
      ctx.lineTo(cx - eyeDx + 3, cy);
      ctx.moveTo(cx + eyeDx - 3, cy);
      ctx.lineTo(cx + eyeDx + 3, cy);
      ctx.stroke();
    } else {
      ctx.beginPath();
      ctx.arc(cx - eyeDx, cy, 2.4, 0, Math.PI * 2);
      ctx.arc(cx + eyeDx, cy, 2.4, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.lineWidth = 1.95;
    if (mood === "source") {
      ctx.beginPath();
      ctx.arc(cx - 2.6, cy + 2.4, 2.6, 0.1 * Math.PI, 0.9 * Math.PI);
      ctx.arc(cx + 2.6, cy + 2.4, 2.6, 0.1 * Math.PI, 0.9 * Math.PI);
      ctx.stroke();
    } else {
      ctx.beginPath();
      ctx.arc(cx, cy + 1.8, 3.8, 0.18 * Math.PI, 0.82 * Math.PI);
      ctx.stroke();
    }
  }
  ctx.restore();
}

/** 2-keyframe discrete sweat-drop + anime panic ticks */
function drawSweatAndPanic(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  step2: number
) {
  ctx.save();
  ctx.translate(x, y + (step2 === 1 ? 3 : 0));
  ctx.beginPath();
  ctx.moveTo(0, -8);
  ctx.bezierCurveTo(4.6, -1.5, 5.0, 4.5, 0, 5.5);
  ctx.bezierCurveTo(-5.0, 4.5, -4.6, -1.5, 0, -8);
  ctx.closePath();
  ctx.fillStyle = "#b8e1ff";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.75;
  ctx.lineJoin = "round";
  ctx.stroke();
  ctx.restore();

  // Crisp 2-frame panic accent lines popping above top-right on frame 1
  if (step2 === 1) {
    ctx.save();
    ctx.strokeStyle = "#e54d42";
    ctx.lineWidth = 1.8;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(x + 9, y - 10);
    ctx.lineTo(x + 14, y - 15);
    ctx.moveTo(x + 12, y - 4);
    ctx.lineTo(x + 18, y - 6);
    ctx.stroke();
    ctx.restore();
  }
}

/** 3-keyframe checkered flag held by destination host */
function drawCuteFlag(
  ctx: CanvasRenderingContext2D,
  anchorX: number,
  anchorY: number,
  step3: number,
  celebrating: boolean
) {
  ctx.save();
  ctx.translate(anchorX, anchorY);
  const angles = celebrating ? [-0.14, 0.04, 0.16] : [-0.03, 0.02, 0.05];
  ctx.rotate(angles[step3 % 3]);
  ctx.strokeStyle = INK;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  // Tiny stick arm + round hand
  ctx.lineWidth = 2.0;
  ctx.beginPath();
  ctx.moveTo(-2, 4);
  ctx.quadraticCurveTo(6, 4, 11, -1);
  ctx.stroke();

  ctx.fillStyle = "#fffdf8";
  ctx.beginPath();
  ctx.arc(11, -1, 3.1, 0, Math.PI * 2);
  ctx.fill();
  ctx.stroke();

  // Flag pole
  ctx.lineWidth = 2.15;
  ctx.beginPath();
  ctx.moveTo(10, 8);
  ctx.lineTo(15, -24);
  ctx.stroke();

  // Checkered cloth with 3-frame flutter wave
  ctx.save();
  ctx.translate(15, -24);
  const clothTilt = [0.12, 0.22, 0.16][step3 % 3];
  ctx.rotate(clothTilt);
  const fw = 22;
  const fh = 15;
  ctx.fillStyle = "#fffdf8";
  ctx.fillRect(0, 0, fw, fh);
  ctx.fillStyle = celebrating ? "#15803d" : INK;
  const cw = fw / 3;
  const ch = fh / 2;
  for (let r = 0; r < 2; r++) {
    for (let c = 0; c < 3; c++) {
      if ((r + c) % 2 === 0) {
        ctx.fillRect(c * cw, r * ch, cw, ch);
      }
    }
  }
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.85;
  ctx.strokeRect(0, 0, fw, fh);
  ctx.restore();

  ctx.restore();
}

/** Cute golden crown with 2-frame twinkle for STP Root Bridge */
export function drawMiniCrown(
  ctx: CanvasRenderingContext2D,
  cx: number,
  y: number,
  step2: number
) {
  ctx.save();
  ctx.translate(cx, y + (step2 === 1 ? -1.5 : 0));
  ctx.beginPath();
  ctx.moveTo(-10, 4);
  ctx.lineTo(-12, -5);
  ctx.lineTo(-4, 0);
  ctx.lineTo(0, -8);
  ctx.lineTo(4, 0);
  ctx.lineTo(12, -5);
  ctx.lineTo(10, 4);
  ctx.closePath();
  ctx.fillStyle = "#fde68a";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.75;
  ctx.lineJoin = "round";
  ctx.stroke();

  // Tiny 2-frame sparkle dot on crown tip
  if (step2 === 1) {
    ctx.strokeStyle = "#d97706";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(0, -14);
    ctx.lineTo(0, -11);
    ctx.moveTo(-14, -9);
    ctx.lineTo(-12, -7);
    ctx.moveTo(14, -9);
    ctx.lineTo(12, -7);
    ctx.stroke();
  }
  ctx.restore();
}

/**
 * Free-standing B1-Beta Cute Host Character with 3-frame flipbook life!
 */
export function drawCuteHostCharacter(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  opts: {
    mood: DeviceMood;
    selected: boolean;
    hovered: boolean;
    hasFlag: boolean;
    blink: boolean;
    frameTick: number;
  }
) {
  const { mood, selected, hovered, hasFlag, blink, frameTick } = opts;
  const step2 = frameTick % 2;
  const step3 = frameTick % 3;
  const bounceY = mood === "happy" ? (step2 === 1 ? -3 : 0) : step3 === 1 ? -0.8 : 0;
  const liftY = (hovered ? -2.5 : 0) + bounceY;

  ctx.save();
  ctx.translate(cx, cy + liftY);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  if (selected) {
    drawRoundedRectPath(ctx, -47, -38, 94, 78, 16);
    ctx.fillStyle = "rgba(186, 230, 253, 0.34)";
    ctx.fill();
    ctx.strokeStyle = "#3b82f6";
    ctx.lineWidth = 1.8;
    ctx.setLineDash([5, 4]);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // Stand base & neck
  ctx.fillStyle = "#fffdf8";
  ctx.strokeStyle = INK;
  ctx.lineWidth = 2.25;
  ctx.beginPath();
  ctx.moveTo(-6, 17);
  ctx.lineTo(-8, 25);
  ctx.lineTo(8, 25);
  ctx.lineTo(6, 17);
  ctx.closePath();
  ctx.fill();
  ctx.stroke();
  drawRoundedRectPath(ctx, -19, 24, 38, 6.5, 3.5);
  ctx.fill();
  ctx.stroke();

  // Outer monitor casing
  drawRoundedRectPath(ctx, -39, -30, 78, 50, 10);
  ctx.fillStyle = mood === "worried" ? "#fdf2e3" : "#fffdf8";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 2.35;
  ctx.stroke();

  // Inner pastel screen
  drawRoundedRectPath(ctx, -31.5, -22.5, 63, 35, 5.5);
  ctx.fillStyle =
    mood === "happy"
      ? "#dbf7e8"
      : mood === "worried"
        ? "#fde7dc"
        : "#dff2fc";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.85;
  ctx.stroke();

  // Subtle 2-frame tiny terminal cursor blink in screen corner
  if (mood !== "worried" && step2 === 0) {
    ctx.strokeStyle = "rgba(44, 37, 34, 0.32)";
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    ctx.moveTo(-26, -17);
    ctx.lineTo(-21, -17);
    ctx.stroke();
  }

  drawCuteFace(ctx, 0, -5.5, mood, blink, step2);

  if (mood === "worried") {
    drawSweatAndPanic(ctx, 30, -22, step2);
  }

  if (hasFlag) {
    drawCuteFlag(ctx, 38, -2, step3, mood === "happy");
  }

  // Cute 2-frame celebration stars hovering near happy host
  if (mood === "happy") {
    ctx.strokeStyle = "#15803d";
    ctx.lineWidth = 1.7;
    const sx = -42;
    const sy = -28 + (step2 === 1 ? -3 : 0);
    ctx.beginPath();
    ctx.moveTo(sx - 4, sy);
    ctx.lineTo(sx + 4, sy);
    ctx.moveTo(sx, sy - 4);
    ctx.lineTo(sx, sy + 4);
    ctx.stroke();
  }

  ctx.restore();
}

/**
 * Free-standing B1-Beta Cute Switch Character with 3-frame twinkling port LEDs!
 */
export function drawCuteSwitchCharacter(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  opts: {
    mood: DeviceMood;
    selected: boolean;
    hovered: boolean;
    isStpRoot: boolean;
    blink: boolean;
    frameTick: number;
    shiverX?: number;
  }
) {
  const { mood, selected, hovered, isStpRoot, blink, frameTick, shiverX = 0 } = opts;
  const step2 = frameTick % 2;
  const step3 = frameTick % 3;
  const worriedJitter = mood === "worried" && step2 === 1 ? 1.2 : 0;
  const liftY = hovered ? -2.5 : 0;

  ctx.save();
  ctx.translate(cx + shiverX + worriedJitter, cy + liftY);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  if (selected) {
    drawRoundedRectPath(ctx, -49, -34, 98, 68, 16);
    ctx.fillStyle = "rgba(167, 243, 208, 0.34)";
    ctx.fill();
    ctx.strokeStyle = "#0d9488";
    ctx.lineWidth = 1.8;
    ctx.setLineDash([5, 4]);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // Trapezoid-topped rounded switch silhouette
  ctx.beginPath();
  ctx.moveTo(-34, -22);
  ctx.lineTo(34, -22);
  ctx.quadraticCurveTo(39, -22, 40, -16);
  ctx.lineTo(43, 5);
  ctx.quadraticCurveTo(44, 10, 44, 14);
  ctx.lineTo(44, 18);
  ctx.quadraticCurveTo(44, 24, 37, 24);
  ctx.lineTo(-37, 24);
  ctx.quadraticCurveTo(-44, 24, -44, 18);
  ctx.lineTo(-44, 14);
  ctx.quadraticCurveTo(-44, 10, -43, 5);
  ctx.lineTo(-40, -16);
  ctx.quadraticCurveTo(-39, -22, -34, -22);
  ctx.closePath();

  ctx.fillStyle =
    mood === "worried"
      ? "#fdf2e3"
      : mood === "happy"
        ? "#c6f3dc"
        : "#d4f3e5";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 2.35;
  ctx.stroke();

  // Cream lower front bezel strip
  ctx.save();
  ctx.beginPath();
  ctx.moveTo(-42.5, 9);
  ctx.lineTo(42.5, 9);
  ctx.lineTo(42.5, 18);
  ctx.quadraticCurveTo(42.5, 22.8, 37, 22.8);
  ctx.lineTo(-37, 22.8);
  ctx.quadraticCurveTo(-42.5, 22.8, -42.5, 18);
  ctx.closePath();
  ctx.fillStyle = "#fffdf8";
  ctx.fill();
  ctx.restore();

  ctx.beginPath();
  ctx.moveTo(-43, 8.5);
  ctx.lineTo(43, 8.5);
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.85;
  ctx.stroke();

  // 4 cute twinkling port LED dots (3-keyframe alternating activity blink!)
  const ledPatterns: number[][] = [
    [1, 0, 1, 0],
    [0, 1, 0, 1],
    [1, 1, 0, 1],
  ];
  const pattern = ledPatterns[step3];
  for (let i = 0; i < 4; i++) {
    const px = -24 + i * 16;
    const isLit = pattern[i] === 1;
    ctx.fillStyle =
      mood === "worried" && i === 0
        ? step2 === 1
          ? "#e54d42"
          : "#fca5a5"
        : isLit
          ? "#34d399"
          : INK;
    ctx.beginPath();
    ctx.arc(px, 16, isLit ? 3.0 : 2.5, 0, Math.PI * 2);
    ctx.fill();
    if (isLit || (mood === "worried" && i === 0)) {
      ctx.strokeStyle = INK;
      ctx.lineWidth = 1.1;
      ctx.stroke();
    }
  }

  drawCuteFace(ctx, 0, -7, mood, blink, step2);

  if (isStpRoot) {
    drawMiniCrown(ctx, 0, -29, step2);
  }

  if (mood === "worried") {
    drawSweatAndPanic(ctx, 29, -15, step2);
  }

  ctx.restore();
}

/**
 * Free-standing B1-Beta Cute Router Character with 3-frame wiggling antennas & signal arcs!
 */
export function drawCuteRouterCharacter(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  opts: {
    mood: DeviceMood;
    selected: boolean;
    hovered: boolean;
    blink: boolean;
    frameTick: number;
    shiverX?: number;
  }
) {
  const { mood, selected, hovered, blink, frameTick, shiverX = 0 } = opts;
  const step2 = frameTick % 2;
  const step3 = frameTick % 3;
  const liftY = hovered ? -2.5 : 0;

  ctx.save();
  ctx.translate(cx + shiverX, cy + liftY);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  if (selected) {
    drawRoundedRectPath(ctx, -46, -38, 92, 72, 16);
    ctx.fillStyle = "rgba(254, 215, 170, 0.40)";
    ctx.fill();
    ctx.strokeStyle = "#ea580c";
    ctx.lineWidth = 1.8;
    ctx.setLineDash([5, 4]);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // 3-keyframe playful antenna tilt
  const antDx = [-2, 0, 2.5][step3];
  ctx.strokeStyle = INK;
  ctx.lineWidth = 2.15;
  ctx.beginPath();
  ctx.moveTo(-16, -15);
  ctx.quadraticCurveTo(-21, -24, -24 - antDx, -31);
  ctx.moveTo(16, -15);
  ctx.quadraticCurveTo(21, -24, 24 + antDx, -31);
  ctx.stroke();

  const leftTipX = -24 - antDx;
  const rightTipX = 24 + antDx;

  ctx.fillStyle = mood === "worried" ? "#f38370" : "#fde68a";
  ctx.beginPath();
  ctx.arc(leftTipX, -31.5, 3.5, 0, Math.PI * 2);
  ctx.fill();
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(rightTipX, -31.5, 3.5, 0, Math.PI * 2);
  ctx.fill();
  ctx.stroke();

  // 3-keyframe cute doodle signal arcs `(( ))` radiating from right antenna tip!
  if (step3 >= 1) {
    ctx.strokeStyle = mood === "worried" ? "#e54d42" : "#ea580c";
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    ctx.arc(rightTipX + 2, -32, 6, -0.7, 0.5);
    ctx.stroke();
    if (step3 === 2) {
      ctx.beginPath();
      ctx.arc(rightTipX + 2, -32, 10, -0.75, 0.55);
      ctx.stroke();
    }
  }

  // Rounded router chassis
  drawRoundedRectPath(ctx, -38, -16, 76, 40, 15);
  ctx.fillStyle = mood === "worried" ? "#fdf2e3" : "#fde7d2";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 2.35;
  ctx.stroke();

  drawCuteFace(ctx, 0, 2, mood, blink, step2);

  // 2-frame twinkling status LED
  ctx.fillStyle = step2 === 0 ? "#4ade80" : "#fde68a";
  ctx.beginPath();
  ctx.arc(-24, 15, 2.4, 0, Math.PI * 2);
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.2;
  ctx.stroke();

  if (mood === "worried") {
    drawSweatAndPanic(ctx, 29, -11, step2);
  }

  ctx.restore();
}

/**
 * 2-keyframe classic flipbook flapping winged envelope packet!
 */
export function drawWingedPacket(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  label: string,
  fillColor: string,
  wingUp: boolean,
  tilt = 0
) {
  ctx.save();
  ctx.translate(x, y + (wingUp ? -2 : 1.5));
  ctx.rotate(tilt + (wingUp ? -0.05 : 0.05));
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  const wingAngle = wingUp ? -0.35 : 0.32;

  // Left cartoon wing
  ctx.save();
  ctx.translate(-16, -2);
  ctx.rotate(wingAngle);
  ctx.beginPath();
  ctx.moveTo(0, 0);
  ctx.quadraticCurveTo(-13, -11, -17, -4);
  ctx.quadraticCurveTo(-14, 1, -9, 0);
  ctx.quadraticCurveTo(-13, 5, -7, 6);
  ctx.quadraticCurveTo(-3, 5, 0, 2);
  ctx.closePath();
  ctx.fillStyle = "#fffdf8";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.8;
  ctx.stroke();
  ctx.restore();

  // Right cartoon wing
  ctx.save();
  ctx.translate(16, -2);
  ctx.rotate(-wingAngle);
  ctx.beginPath();
  ctx.moveTo(0, 0);
  ctx.quadraticCurveTo(13, -11, 17, -4);
  ctx.quadraticCurveTo(14, 1, 9, 0);
  ctx.quadraticCurveTo(13, 5, 7, 6);
  ctx.quadraticCurveTo(3, 5, 0, 2);
  ctx.closePath();
  ctx.fillStyle = "#fffdf8";
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.8;
  ctx.stroke();
  ctx.restore();

  // Envelope body
  drawRoundedRectPath(ctx, -17, -11, 34, 22, 4);
  ctx.fillStyle = fillColor;
  ctx.fill();
  ctx.strokeStyle = INK;
  ctx.lineWidth = 2.0;
  ctx.stroke();

  // Envelope V-flap
  ctx.beginPath();
  ctx.moveTo(-16, -10);
  ctx.lineTo(0, 1.5);
  ctx.lineTo(16, -10);
  ctx.strokeStyle = INK;
  ctx.lineWidth = 1.65;
  ctx.stroke();

  // Tiny label right above envelope
  ctx.font = '800 10px "Cascadia Code", Consolas, monospace';
  ctx.fillStyle = INK;
  ctx.textAlign = "center";
  ctx.textBaseline = "bottom";
  ctx.fillText(label, 0, -13);

  ctx.restore();
}

export function drawRedCrayonCross(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  scale = 1
) {
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(scale, scale);
  ctx.strokeStyle = "#e54d42";
  ctx.lineWidth = 4.0;
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.moveTo(-8.5, -8.5);
  ctx.lineTo(8.5, 8.5);
  ctx.moveTo(8.5, -8.5);
  ctx.lineTo(-8.5, 8.5);
  ctx.stroke();
  ctx.restore();
}

export function drawDoodleSparkles(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  color: string,
  phase: number
) {
  ctx.save();
  ctx.translate(cx, cy);
  ctx.strokeStyle = color;
  ctx.lineWidth = 2.0;
  ctx.lineCap = "round";
  const r = 10 + phase * 10;
  ctx.globalAlpha = Math.max(0, 1 - phase);
  for (let i = 0; i < 6; i++) {
    const a = (i * Math.PI) / 3 + 0.3;
    ctx.beginPath();
    ctx.moveTo(Math.cos(a) * r * 0.55, Math.sin(a) * r * 0.55);
    ctx.lineTo(Math.cos(a) * r, Math.sin(a) * r);
    ctx.stroke();
  }
  ctx.restore();
}

export function fitText(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number
): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (ctx.measureText(text.slice(0, mid) + "…").width <= maxWidth) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  return lo > 0 ? text.slice(0, lo) + "…" : "…";
}
