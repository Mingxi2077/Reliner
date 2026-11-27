import React from "react";
import { Trash2 } from "lucide-react";
import type { LinkSpec, NativeLabDocument } from "../../model/document";
import { findPortOwner } from "../../model/document";
import { NumericInput } from "./NumericInput";

interface LinkFormProps {
  doc: NativeLabDocument;
  link: LinkSpec;
  onChangeDoc: (next: NativeLabDocument) => void;
  onDeleteLink: (linkId: string) => void;
  onJumpToPort: (deviceId: string, portId: string) => void;
  onSaveAndRerun?: () => void;
  rerunLabel?: string;
}

export const LinkForm: React.FC<LinkFormProps> = ({
  doc,
  link,
  onChangeDoc,
  onDeleteLink,
  onJumpToPort,
  onSaveAndRerun,
  rerunLabel = "保存并重跑",
}) => {
  const isLegacy = doc.format_version === 1;
  const ownerA = findPortOwner(doc, link.ports[0]);
  const ownerB = findPortOwner(doc, link.ports[1]);

  const isSwitchPair =
    ownerA?.device.kind === "switch" && ownerB?.device.kind === "switch";
  const pairKey = (devA: string, devB: string) =>
    devA < devB ? `${devA}|${devB}` : `${devB}|${devA}`;
  const thisPair =
    ownerA && ownerB ? pairKey(ownerA.device.id, ownerB.device.id) : null;
  const isAlreadyPrimary = doc.links.some((l) => l.standby_for === link.id);
  const protectedPrimaries = new Set(
    doc.links
      .filter((l) => l.id !== link.id && l.standby_for)
      .map((l) => l.standby_for!)
  );
  const standbyCandidates = isSwitchPair && !isAlreadyPrimary
    ? doc.links.filter((other) => {
        if (other.id === link.id) return false;
        if (other.standby_for) return false;
        if (protectedPrimaries.has(other.id)) return false;
        const oA = findPortOwner(doc, other.ports[0]);
        const oB = findPortOwner(doc, other.ports[1]);
        if (!oA || !oB) return false;
        return pairKey(oA.device.id, oB.device.id) === thisPair;
      })
    : [];

  const updateLink = (next: LinkSpec) => {
    onChangeDoc({
      ...doc,
      links: doc.links.map((item) => (item.id === link.id ? next : item)),
    });
  };

  return (
    <div>
      <div className="card">
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            marginBottom: 10,
          }}
        >
          <div>
            <span className="pill pill-neutral" style={{ marginRight: 6 }}>
              物理链路
            </span>
            <strong className="mono">{link.id}</strong>
          </div>
          <button
            className="btn btn-xs btn-danger"
            onClick={() => onDeleteLink(link.id)}
          >
            <Trash2 size={12} /> 删除链路
          </button>
        </div>

        <div className="panel-section-title">两端接口</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 10 }}>
          {[
            { portId: link.ports[0], owner: ownerA },
            { portId: link.ports[1], owner: ownerB },
          ].map((end, idx) => (
            <div
              key={idx}
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "center",
                padding: "6px 8px",
                borderRadius: 6,
                background: "var(--bg-subtle)",
                border: "1px solid var(--border-subtle)",
              }}
            >
              <span
                className="mono small"
                style={{
                  minWidth: 0,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {end.owner
                  ? `${end.owner.device.name}/${end.owner.portName} (${end.portId})`
                  : end.portId}
              </span>
              {end.owner && (
                <button
                  className="btn btn-xs"
                  style={{ flexShrink: 0 }}
                  onClick={() => onJumpToPort(end.owner!.device.id, end.portId)}
                >
                  配置端口
                </button>
              )}
            </div>
          ))}
        </div>

        <div className="field">
          <label>传播延迟 delay_ns（正整数纳秒）</label>
          <input
            className="mono"
            value={link.delay_ns}
            onChange={(e) => updateLink({ ...link, delay_ns: e.target.value })}
          />
        </div>

        {!isLegacy && (
          <div className="field">
            <label>主备保护 standby_for（可选，指定同对交换机间的主链路 ID）</label>
            {isSwitchPair || link.standby_for ? (
              <>
                <select
                  value={link.standby_for ?? ""}
                  onChange={(e) =>
                    updateLink({
                      ...link,
                      standby_for: e.target.value === "" ? undefined : e.target.value,
                    })
                  }
                >
                  <option value="">无（普通独立链路）</option>
                  {link.standby_for &&
                    !standbyCandidates.some((c) => c.id === link.standby_for) && (
                      <option value={link.standby_for}>
                        {link.standby_for}（当前配置）
                      </option>
                    )}
                  {standbyCandidates.map((other) => (
                    <option key={other.id} value={other.id}>
                      {other.id}
                    </option>
                  ))}
                </select>
                {doc.scenario?.stp_convergence_ns && (
                  <span className="small muted">
                    提示：当前拓扑已启用 STP，请勿与主备保护链路混用。
                  </span>
                )}
              </>
            ) : (
              <div className="small muted">
                主备保护仅适用于同对交换机之间的平行链路。
              </div>
            )}
          </div>
        )}
      </div>

      {/* Transmission rate & finite send queue clearly separated per README */}
      {!isLegacy && (
        <div className="card">
          <div className="panel-section-title">
            <span>链路速率与有限发送队列（transmission）</span>
            {link.transmission ? (
              <button
                className="btn btn-xs btn-danger"
                onClick={() => updateLink({ ...link, transmission: undefined })}
              >
                关闭限速
              </button>
            ) : (
              <button
                className="btn btn-xs"
                onClick={() =>
                  updateLink({
                    ...link,
                    transmission: { rate_bps: "1000000000", queue_frames: 1 },
                  })
                }
              >
                启用限速与队列
              </button>
            )}
          </div>

          {link.transmission ? (
            <div className="form-grid">
              <div className="field">
                <label>发送速率 rate_bps (bit/s)</label>
                <input
                  className="mono"
                  value={link.transmission.rate_bps}
                  onChange={(e) =>
                    updateLink({
                      ...link,
                      transmission: {
                        ...link.transmission!,
                        rate_bps: e.target.value,
                      },
                    })
                  }
                />
              </div>
              <div className="field">
                <label>每方向等待帧数 queue_frames</label>
                <NumericInput
                  min={0}
                  max={65535}
                  value={link.transmission.queue_frames}
                  onValueChange={(queue_frames) =>
                    updateLink({
                      ...link,
                      transmission: {
                        ...link.transmission!,
                        queue_frames,
                      },
                    })
                  }
                />
              </div>
            </div>
          ) : (
            <div className="small muted">
              未配置传输速率与队列，沿用固定传播延迟模型。
            </div>
          )}

          {onSaveAndRerun && (
            <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 8 }}>
              <button
                type="button"
                className="btn btn-sm btn-coral"
                onClick={onSaveAndRerun}
              >
                {rerunLabel}
              </button>
            </div>
          )}
        </div>
      )}

      {isLegacy && onSaveAndRerun && (
        <div style={{ display: "flex", justifyContent: "flex-end", marginBottom: 8 }}>
          <button
            type="button"
            className="btn btn-sm btn-coral"
            onClick={onSaveAndRerun}
          >
            {rerunLabel}
          </button>
        </div>
      )}
    </div>
  );
};
