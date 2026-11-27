import React from "react";

/** B1-Beta cute smiling rounded mascot mark with stepped flipbook blink */
export const SketchBrandMark: React.FC<{ size?: number }> = ({ size = 28 }) => (
  <svg className="mascot-bob" width={size} height={size} viewBox="0 0 34 34" fill="none" aria-hidden="true">
    <path
      d="M8 10 C8 6.5, 11.5 5, 17 5 C22.5 5, 26 6.5, 26 10 L28 21 C28.5 26, 25 29, 17 29 C9 29, 5.5 26, 6 21 Z"
      fill="#fffdf8"
      stroke="#2c2522"
      strokeWidth="2.2"
      strokeLinejoin="round"
    />
    <g className="mascot-eyes">
      <circle cx="12.5" cy="16.5" r="1.7" fill="#2c2522" />
      <circle cx="21.5" cy="16.5" r="1.7" fill="#2c2522" />
    </g>
    <path
      d="M14.5 18.5 Q17 21.2 19.5 18.5"
      stroke="#2c2522"
      strokeWidth="1.9"
      strokeLinecap="round"
      fill="none"
    />
  </svg>
);

/** Pure flat B1-Beta rounded-nib Host icon */
export const SketchHostSvg: React.FC<{ size?: number }> = ({ size = 26 }) => (
  <svg width={size} height={size} viewBox="0 0 34 34" fill="none" aria-hidden="true">
    <rect
      x="4"
      y="5"
      width="26"
      height="19"
      rx="4"
      fill="#dff2fc"
      stroke="#2c2522"
      strokeWidth="2.1"
    />
    <circle cx="13" cy="14" r="1.3" fill="#2c2522" />
    <circle cx="21" cy="14" r="1.3" fill="#2c2522" />
    <path d="M15 16 Q17 18 19 16" stroke="#2c2522" strokeWidth="1.6" strokeLinecap="round" />
    <path
      d="M15 24 L14 28.5 M19 24 L20 28.5 M10.5 29 H23.5"
      stroke="#2c2522"
      strokeWidth="2.1"
      strokeLinecap="round"
    />
  </svg>
);

/** Pure flat B1-Beta rounded-nib Switch icon */
export const SketchSwitchSvg: React.FC<{ size?: number }> = ({ size = 26 }) => (
  <svg width={size} height={size} viewBox="0 0 34 34" fill="none" aria-hidden="true">
    <path
      d="M7 9 H27 L30 20 V24 C30 25.6 28.6 27 27 27 H7 C5.4 27 4 25.6 4 24 V20 Z"
      fill="#d4f3e5"
      stroke="#2c2522"
      strokeWidth="2.1"
      strokeLinejoin="round"
    />
    <path d="M5 20 H29" stroke="#2c2522" strokeWidth="1.8" />
    <circle cx="10" cy="23.5" r="1.3" fill="#2c2522" />
    <circle cx="14.7" cy="23.5" r="1.3" fill="#2c2522" />
    <circle cx="19.3" cy="23.5" r="1.3" fill="#2c2522" />
    <circle cx="24" cy="23.5" r="1.3" fill="#2c2522" />
    <circle cx="13.5" cy="14" r="1.2" fill="#2c2522" />
    <circle cx="20.5" cy="14" r="1.2" fill="#2c2522" />
    <path d="M15.2 15.6 Q17 17.4 18.8 15.6" stroke="#2c2522" strokeWidth="1.5" strokeLinecap="round" />
  </svg>
);

/** Pure flat B1-Beta rounded-nib Router icon */
export const SketchRouterSvg: React.FC<{ size?: number }> = ({ size = 26 }) => (
  <svg width={size} height={size} viewBox="0 0 34 34" fill="none" aria-hidden="true">
    <path d="M11 12 L8 6 M23 12 L26 6" stroke="#2c2522" strokeWidth="2" strokeLinecap="round" />
    <circle cx="8" cy="5.5" r="1.8" fill="#fde68a" stroke="#2c2522" strokeWidth="1.5" />
    <circle cx="26" cy="5.5" r="1.8" fill="#fde68a" stroke="#2c2522" strokeWidth="1.5" />
    <rect
      x="4"
      y="12"
      width="26"
      height="15"
      rx="7"
      fill="#fde7d2"
      stroke="#2c2522"
      strokeWidth="2.1"
    />
    <circle cx="13" cy="19" r="1.3" fill="#2c2522" />
    <circle cx="21" cy="19" r="1.3" fill="#2c2522" />
    <path d="M15.2 21 Q17 22.8 18.8 21" stroke="#2c2522" strokeWidth="1.5" strokeLinecap="round" />
  </svg>
);

/** Pure flat B1-Beta rounded-nib Wire / Cable icon */
export const SketchWireSvg: React.FC<{ size?: number }> = ({ size = 24 }) => (
  <svg width={size} height={size} viewBox="0 0 34 34" fill="none" aria-hidden="true">
    <path
      d="M5 17 C9 9, 13 9, 17 17 C21 25, 25 25, 29 17"
      stroke="#2c2522"
      strokeWidth="2.2"
      strokeLinecap="round"
      fill="none"
    />
    <circle cx="5" cy="17" r="2.6" fill="#dff2fc" stroke="#2c2522" strokeWidth="1.8" />
    <circle cx="29" cy="17" r="2.6" fill="#d4f3e5" stroke="#2c2522" strokeWidth="1.8" />
  </svg>
);

/** Pure flat B1-Beta rounded-nib Probe / Radar icon */
export const SketchProbeSvg: React.FC<{ size?: number }> = ({ size = 24 }) => (
  <svg width={size} height={size} viewBox="0 0 34 34" fill="none" aria-hidden="true">
    <circle cx="17" cy="15" r="8.5" fill="#fff0a8" stroke="#2c2522" strokeWidth="2.0" />
    <circle cx="17" cy="15" r="2.5" fill="#2c2522" />
    <path d="M17 17.5 V29 M12 29 H22" stroke="#2c2522" strokeWidth="2.1" strokeLinecap="round" />
  </svg>
);

/** Pure flat B1-Beta Welcome Banner Illustration */
export const WelcomeSketchBanner: React.FC = () => (
  <svg
    viewBox="0 0 540 112"
    style={{ width: "100%", maxWidth: 500, height: "auto", display: "block", margin: "0 auto 8px" }}
    fill="none"
    aria-hidden="true"
  >
    {/* Connecting wires */}
    <path d="M96 64 H196 M282 64 H356 M442 64 H462" stroke="#2c2522" strokeWidth="2.3" strokeLinecap="round" />
    {/* Dotted arc for winged ARP envelope */}
    <path
      d="M236 34 Q 294 8, 350 48"
      stroke="#2c2522"
      strokeWidth="1.9"
      strokeDasharray="5 5"
      strokeLinecap="round"
      fill="none"
    />
    {/* Cute winged ARP envelope with 2-frame CSS flipbook float */}
    <g className="banner-arp-float">
      <g transform="translate(290, 24) rotate(8)">
      <path d="M-14 -1 Q-25 -10 -27 -2 Q-23 3 -14 2 Z" fill="#fffdf8" stroke="#2c2522" strokeWidth="1.7" />
      <path d="M14 -1 Q25 -10 27 -2 Q23 3 14 2 Z" fill="#fffdf8" stroke="#2c2522" strokeWidth="1.7" />
      <rect x="-14" y="-9" width="28" height="18" rx="3" fill="#fff0a8" stroke="#2c2522" strokeWidth="1.9" />
      <path d="M-13 -8 L0 2 L13 -8" stroke="#2c2522" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
      <text x="0" y="-12" textAnchor="middle" fontSize="9.5" fontWeight="800" fill="#2c2522" fontFamily="Consolas, monospace">ARP</text>
      </g>
    </g>

    {/* PC-A cute monitor */}
    <g transform="translate(56, 62)">
      <rect x="-32" y="-24" width="64" height="42" rx="8" fill="#fffdf8" stroke="#2c2522" strokeWidth="2.2" />
      <rect x="-25" y="-17" width="50" height="28" rx="4" fill="#dff2fc" stroke="#2c2522" strokeWidth="1.7" />
      <circle cx="-9" cy="-4" r="2.2" fill="#2c2522" />
      <circle cx="9" cy="-4" r="2.2" fill="#2c2522" />
      <path d="M-4 -1 Q0 3 4 -1" stroke="#2c2522" strokeWidth="1.8" strokeLinecap="round" />
      <path d="M-5 18 L-6 24 M5 18 L6 24 M-14 24 H14" stroke="#2c2522" strokeWidth="2.1" strokeLinecap="round" />
      <text x="0" y="38" textAnchor="middle" fontSize="11" fontWeight="800" fill="#2c2522">PC-A</text>
    </g>

    {/* SW1 happy mint switch */}
    <g transform="translate(238, 64)">
      <path d="M-32 -18 H32 L38 6 V16 C38 19 35 21 32 21 H-32 C-35 21 -38 19 -38 16 V6 Z" fill="#d4f3e5" stroke="#2c2522" strokeWidth="2.2" strokeLinejoin="round" />
      <path d="M-37 7 H37" stroke="#2c2522" strokeWidth="1.8" />
      <circle cx="-18" cy="14" r="2.3" fill="#2c2522" />
      <circle cx="-6" cy="14" r="2.3" fill="#2c2522" />
      <circle cx="6" cy="14" r="2.3" fill="#2c2522" />
      <circle cx="18" cy="14" r="2.3" fill="#2c2522" />
      <circle cx="-9" cy="-6" r="2.1" fill="#2c2522" />
      <circle cx="9" cy="-6" r="2.1" fill="#2c2522" />
      <path d="M-4 -4 Q0 0 4 -4" stroke="#2c2522" strokeWidth="1.8" strokeLinecap="round" />
      <text x="0" y="36" textAnchor="middle" fontSize="11" fontWeight="800" fill="#2c2522">SW1</text>
    </g>

    {/* SW2 cute worried switch + red crayon X */}
    <g transform="translate(396, 64)">
      <path d="M-32 -18 H32 L38 6 V16 C38 19 35 21 32 21 H-32 C-35 21 -38 19 -38 16 V6 Z" fill="#fdf2e3" stroke="#2c2522" strokeWidth="2.2" strokeLinejoin="round" />
      <path d="M-37 7 H37" stroke="#2c2522" strokeWidth="1.8" />
      <circle cx="-18" cy="14" r="2.3" fill="#2c2522" />
      <circle cx="-6" cy="14" r="2.3" fill="#2c2522" />
      <circle cx="6" cy="14" r="2.3" fill="#2c2522" />
      <circle cx="18" cy="14" r="2.3" fill="#2c2522" />
      <path d="M-13 -5 Q-9 -9 -5 -5 M5 -5 Q9 -9 13 -5 M-3 0 Q0 -3 3 0" stroke="#2c2522" strokeWidth="1.8" strokeLinecap="round" />
      <path d="M24 -18 C28 -12 28 -7 24 -6 C20 -7 20 -12 24 -18 Z" fill="#b8e1ff" stroke="#2c2522" strokeWidth="1.5" />
      <path d="M-45 -6 L-31 8 M-31 -6 L-45 8" stroke="#e54d42" strokeWidth="3.8" strokeLinecap="round" />
      <text x="0" y="36" textAnchor="middle" fontSize="11" fontWeight="800" fill="#2c2522">SW2</text>
    </g>

    {/* PC-B holding flag */}
    <g transform="translate(490, 62)">
      <rect x="-28" y="-22" width="56" height="38" rx="7" fill="#fffdf8" stroke="#2c2522" strokeWidth="2.2" />
      <rect x="-21" y="-15" width="42" height="24" rx="4" fill="#dff2fc" stroke="#2c2522" strokeWidth="1.7" />
      <circle cx="-8" cy="-4" r="2" fill="#2c2522" />
      <circle cx="8" cy="-4" r="2" fill="#2c2522" />
      <circle cx="0" cy="0" r="1.3" fill="#2c2522" />
      <path d="M-5 16 L-6 22 M5 16 L6 22 M-13 22 H13" stroke="#2c2522" strokeWidth="2.1" strokeLinecap="round" />
      <text x="0" y="36" textAnchor="middle" fontSize="11" fontWeight="800" fill="#2c2522">PC-B</text>
    </g>
  </svg>
);

export function getExampleCategory(id: string): { num: string; tag: string; tone: string } {
  switch (id) {
    case "vlan-lab":
      return { num: "01", tag: "二层 VLAN", tone: "tag-teal" };
    case "native-vlan-lab":
      return { num: "02", tag: "Native VLAN", tone: "tag-teal" };
    case "ipv4-lab":
      return { num: "03", tag: "IPv4 / 断链", tone: "tag-blue" };
    case "failover-lab":
      return { num: "04", tag: "主备切换", tone: "tag-amber" };
    case "mtu-lab":
      return { num: "05", tag: "出口 MTU", tone: "tag-orange" };
    case "congestion-lab":
      return { num: "06", tag: "限速队列", tone: "tag-red" };
    case "routing-lab":
      return { num: "07", tag: "动态路由", tone: "tag-orange" };
    case "stp-lab":
      return { num: "08", tag: "STP 选路", tone: "tag-teal" };
    case "acl-lab":
      return { num: "09", tag: "IPv4 ACL", tone: "tag-red" };
    case "campus-lab":
      return { num: "10", tag: "综合关卡", tone: "tag-amber" };
    case "stack-static-lab":
      return { num: "11", tag: "基准连通", tone: "tag-green" };
    default:
      return { num: "··", tag: "自定义", tone: "tag-neutral" };
  }
}
