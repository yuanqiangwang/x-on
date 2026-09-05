# ADR 0001 — 不透明窗口 + DOM 哑光毛玻璃

- **Status**: Accepted
- **Date**: 2026-09-05

## Context

Raycast / uTools 的经典观感是"毛玻璃模糊桌面"。直觉做法是透明窗口 + `backdrop-filter: blur()`。经拷问与验证，此路在 Windows 上不成立：

- WebView2 的 `backdrop-filter` 只采样**同一文档内**位于其后的元素，采不到 OS 窗口背后的桌面/其他窗口——对"模糊桌面"**无效**。
- 真透明窗口（`transparent: true`）在 Windows + WebView2 上有已知问题：黑色伪影、点击穿透、重绘 bug，且与 `alwaysOnTop` / `skipTaskbar` 组合易导致无法获得焦点。
- 要做真正的 Acrylic 需走 DWM `DWMWA_SYSTEMBACKDROP_TYPE` + 原生整合，复杂度高，并与透明窗口存在冲突。

## Decision

v1 采用**不透明窗口**，毛玻璃观感用 DOM 绘制：深色半透明渐变作为基底、内阴影 + 细边框，对面板内部元素轻模糊，营造"哑光"质感，**不追求真正模糊桌面**。

- 常驻后台（唤醒）模型：仅唤出时显示。
- 补偿手段：置顶（`alwaysOnTop`）+ `skipTaskbar` + 失焦（`Focused(false)`）隐藏，保持"来去如风"的使用感。

## Consequences

- ✅ 规避全部 Windows 透明/合成伪影与焦点问题；渲染稳定、性能佳。
- ⚠️ 观感为"哑光"，非真正模糊桌面——与 Raycast 的丙烯酸质感有差距。
- 🔄 若二期要上真 Acrylic，需重做渲染层（改透明 + DWM 整合），成本明显——因此记录为 ADR。

## 备选路径（均未采用）

- 透明窗口 + CSS `backdrop-filter`：对桌面无效，已否定。
- 真透明 + DWM Acrylic：复杂度高、与透明窗口冲突，留二期评估。
