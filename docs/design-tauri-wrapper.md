# dsh-app — Tauri 包壳整体设计

> ⚠️ **实现已收窄**：本文档描述的"运行时来源"部分（D2、D7–D10、种子闭包、更新通道、
> 变体轴）已被 [spec-external-dsh.md](spec-external-dsh.md) 取代 —— 壳不再自带任何运行时，
> 只负责启动用户指定的 `dsh`。仍然成立的是 D1（载体）、D5（进程与生命周期）、
> D6（壳与运行时的契约）与 §21/§22（launcher 定位）。
>
> 状态：设计草案 v2（待评审）。阶段一评审通过后再进入实现。
> 上游事实来源：`/Users/rqc/r/github.com/deepseek-ai/deepseek-harness` @ 本地 checkout（含只读审计报告 `docs/research/dsh-process-audit.md`）。
> 平台事实来源：Tauri v2 调研报告 `docs/research/tauri-v2-desktop-shell-research.md`（2026-09-16 基线，见附录 A）。
> 本文所有"上游事实"均带文件路径；"设计决定"标注为 D1…D9，便于逐条评审。**v2 相对 v1 的改动集中在 D1（载体）、D6–D9（版本切换）、附录 A。**

---

## 1. 目标与非目标

### 目标

| # | 目标 |
|---|---|
| G1 | 用 Tauri v2 把一个**自包含、签名、可公证**的 macOS `.app`/`.dmg` 交付给终端用户，双击即用，不要求用户懂 Node/pnpm |
| G2 | 同时提供**两个发行变体**：`bundled`（内置 Node 运行时）与 `slim`（依赖外部 Node），二者共用同一套 Rust 壳与前端 |
| G3 | 用户可**自选 preset**：preset 决定"启动哪个 dsh 组合、用哪种载体、跑哪份配置"，而不是把配置写死在壳里 |
| G4 | 全程可用 **Nix** 复现构建与开发环境；正式分发走 **Homebrew**（cask 为主） |
| G5 | **版本策略**：一次安装只承载一个真实后端版本；支持"通道内追新"（master 追最新 master / release 追最新 release tag）与"回到上一版本"，**不做任意版本挑选** |
| G6 | **多 preset 并发托管**：多个 preset 可同时运行（各自独立 profile），UI 先支持"单活动窗口 + 后台托管" |

### 非目标（本阶段明确不做）

- Linux / Windows 发行（上游 Electron 版亦仅支持 mac-arm64 / mac-x64 / win-x64，`apps/desktop/README.md`；Linux desktop 明确 unsupported）。Rust 壳代码保持跨平台，但不承诺产物。
- 替代上游 Electron 桌面版。二者是并行的两种壳，共享 `$DSH_HOME` 中的**产品数据**（会话、设置、凭据），不共享可执行包与 profile。
- 在壳内重新实现 dsh 的 profile / patch / 插件机制。preset 只是一层**薄封装**，解析结果最终必须落到上游的 `--profile` + patch 层语义上。

---

## 2. 上游事实基线（决定设计的硬约束）

| 事实 | 证据 | 对设计的含义 |
|---|---|---|
| `dsh` 已发布到 npm，`latest=0.1.5-rc.1`，`bin.dsh=lib/bin.js`，`node-addon-require-builtin` 是**直接依赖** | `registry.npmjs.org/@deepseek-ai/dsh` | 不需要 vendor 整个 harness 源码即可构建运行时闭包 |
| 运行时要求 `engines: ^22.19.0 \|\| >=24.0.0` | `package.json:8-10` | Node 解析策略必须做版本探测 |
| `dsh web` 在 loader 完全 settle 后向 stdout 打印 `dsh web: http://127.0.0.1:<port>/?token=<32B base64url>` | `packages/bundle/web-app/src/index.ts:252-279`；源码注释明说"**supervisors RPC as soon as they observe the line**" | 上游**已经**为"父进程监督者"设计了就绪信号 —— 壳不需要轮询端口 |
| `/api` 与 `/api/remote.mux` 由 Host/Origin 栅栏 + 签名 HttpOnly Cookie 鉴权；index 用 `?token=` 换 cookie 后 303 到干净 `/` | `packages/client/connection/src/api-request-trust.ts:91-118`、`.agents/notes/implemented/architecture/2026-08-24-browser-token-authentication.md` | **loopback HTTP 载体已经是被设计过的安全载体**，不是"临时方案" |
| 未鉴权面：`/plugins/<id>/client.js`、`/plugins/events`(SSE)、以及所有非 index 静态资源 | `packages/client/modules/src/index.ts:570-575`、`packages/client/hmr/src/index.ts:175-200` | 泄漏的是客户端模块图，不是会话数据；但仍应作为已知风险记录 |
| `web` profile 模板是 `patchReload: 'live'`，会挂 `cordis-plugin-hmr`，后者在无 internals 访问时**抛错**（需要 `node-addon-require-builtin` 平台包或 `--expose-internals`） | `packages/boot/app-boot/src/profile.ts:115-118`、`vendor/hmr/src/index.ts:121` | 打包版**不应**用 `web` 模板；且 Node 解析必须校验 internals 能力 |
| profile 模板只有 `acp / web / headless / sdk / sdk-minimal`；`desktop` 被保留；未知名字报错并提示 `dsh plugin` | `packages/boot/app-boot/src/profile.ts:110-131` | 自建 preset 的 profile 必须由**我们**物化，不能指望自动初始化 |
| `dsh plugin` 是**裸转发 PATH 里的 pnpm**，无 pnpm 则 exit 127；无只读开关 | `apps/cli/src/plugin.ts:134-146` | 插件管理需要我们把内置 pnpm 前置到 PATH（Electron 版正是这么做的） |
| 所有可变状态在 `$DSH_HOME`（默认 `~/.dsh`），启动**不写入安装目录** | `packages/util/home-paths/src/index.ts:12,61-63` | Homebrew root-owned 只读前缀可行 |
| 无 `$DSH_HOME` 级 / profile 级互斥锁；只有文件级 `<file>.lock`（`wx`，无陈旧恢复） | 审计报告 §3、§4 | **单实例锁必须由壳提供** |
| 原生依赖全部预编译：`node-pty`、`koffi`、`node-addon-require-builtin`、`@deepseek-ai/node-addon-system`（mac 下 `bin/system.node`）、`@vscode/ripgrep` | 审计报告 §2 | 无 node-gyp；但**每个 Mach-O 都要重签名** |
| 首次运行**无网络下载** | 审计报告 §2 | 离线开箱可用是可达的 |
| 存在成熟的 `pnpm deploy --legacy --prod` 闭包配方 | `scripts/build-exe-for-python-sdk.ts:289-299`、`scripts/verify-runtime-closure.ts` | 直接复用，不自创配方 |

---

## 3. 总体架构

```
┌──────────────────────────────────────────────────────────────────────────┐
│ dsh-app  (Tauri v2, Rust 主进程 = 壳 + launcher + supervisor)              │
│                                                                          │
│  runtimes/current (单版本) + runtimes/prev (回滚点)                        │
│  ┌──────────────────┐   ┌────────────────┐   ┌──────────────────────┐    │
│  │ Runtime Store    │──▶│ Runtime        │──▶│ LaunchPlan (纯数据)   │    │
│  │ 安装/枚举/激活/回退 │   │ Resolver       │   │ Command + carrier 语义│    │
│  └──────────────────┘   │ (node 探测链)   │   └───────────┬──────────┘    │
│          ▲              └────────────────┘               │               │
│          │                                               ▼               │
│  ┌──────────────────┐   ┌────────────────┐   ┌──────────────────────┐    │
│  │ Preset 解析       │──▶│ 单实例锁 /      │──▶│ Carrier (trait)      │    │
│  │ (用户可选)        │   │ 生命周期编排    │   │ Web | Stdio | Pipe   │    │
│  └──────────────────┘   └────────────────┘   └───────────┬──────────┘    │
│                                                          │               │
│                                              stdout: "dsh web: <url>"     │
│                                                          ▼               │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │ WKWebView ──loads──▶ http://127.0.0.1:<随机端口>/?token=...         │  │
│  └────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────┘
        │ 安装前缀（只读、root-owned、nix/brew 管理）
        ├── Contents/MacOS/dsh-app            Rust 壳
        └── Contents/Resources/
            ├── node/<platform>/<ver>/         共享 Node 安装（bundled 变体）
            ├── pnpm/                          内置 pnpm（插件管理）
            ├── runtimes/<channel>-<ver>/      预装运行时（闭包 + runtime.json）
            └── ui/                            壳自己的启动/错误/preset 页
```

### D1 — 载体（carrier）选型：loopback web carrier（**已由平台事实确认为唯一可行解**）

选 **A：复用上游 web carrier**。理由（前三条是设计偏好，第四条是硬约束）：

1. 上游**主动**为监督者留了就绪信号（stdout 的 `dsh web: <url>` 行），说明"父进程托管 + 解析 URL"是被支持的用法，不是逆向出来的技巧。
2. 鉴权（launch token → HttpOnly Cookie + Host/Origin 栅栏）已经在生产 profile 里跑通并被专门的 Agent Note 论证过；自己造一套自定义协议等于**重新实现一遍别人已经做完的安全工作**。
3. 资产、`window.__DSH_BOOT__`、`/plugins/<id>/client.js`、SSE、`/api/remote.mux`(WS) 全部原样可用，WebView 侧零改动。
4. **[硬约束，见附录 A.1]** Tauri v2 的自定义协议 API 把请求与响应**全量缓冲**（`http::Request<Vec<u8>>` → `Response<Cow<'static,[u8]>>`），流式响应 issue 自 2023 年未解。本项目的核心流量是 SSE 与长连接，**走自定义协议在 v2 上不可行**；除非用 `tauri::ipc::Channel` 重写客户端传输层。

补充：`http://127.0.0.1:<port>` 是 W3C potentially-trustworthy origin，不受 mixed-content 阻断；而自定义 scheme 的 secure-context 状态在 macOS 上没有等价保证（A.2）。loopback 在安全语义上也更稳。

代价与缓解：

| 代价 | 缓解 |
|---|---|
| 会开一个 loopback 端口 | `--port 0` 取 OS 随机端口；仅 `127.0.0.1`（上游拒绝 `--host 0.0.0.0`，`packages/bundle/web-app/src/startup.ts:74-76`） |
| 本机其它进程可访问未鉴权静态面 | 记录为已知风险；随机端口 + 未鉴权面只泄漏模块图；后续可向上游提 issue |
| 端口/进程残留 | 壳负责 SIGTERM→SIGKILL 升级、进程组清理、启动前残留探测。**Tauri 不会替我们回收 Rust 侧 spawn 的子进程**（A.3），这是必须自己实现的部分 |
| WebView 与 loopback 的 origin 关系 | 窗口**直接加载**该 URL（不是从 `tauri://` 去 fetch），因此不涉及 CORS；这也绕开了 A.2 的 secure-context 不确定性 |

**抽象**：Rust 侧定义

```rust
/// 运行时的最终选择，决定 argv[0] 与首参的形状。
enum RuntimeKind { Configured, System, BundledNode, BundledSea }

/// 不论哪种运行时，Launch 层只看「可执行 + 参数 + 环境 + cwd」，
/// 这样 SEA 变体（D2.1）接入时不需要改 carrier / preset / 生命周期。
struct Command { exe: PathBuf, args: Vec<OsString>, env: BTreeMap<String,String>, cwd: PathBuf }

/// 三种 carrier 对应 D3.3 的三种 preset kind；共同点只有「起进程」。
enum LaunchPlan {
    Web    { command: Command, host: String, port: u16 },  // 等到 "dsh web:" URL 行
    Task   { command: Command },                           // 等到退出码，收集输出
    Server { command: Command },                           // 长期运行，仅状态 + 日志
}
```

`Carrier` trait 只有三个方法：`spawn` → `await_ready` → `shutdown`。三种实现对 `await_ready` 的语义不同（URL 行 / 退出 / "已运行即就绪"），这正是 kind 的全部含义。

> **早期版本的 `Pipe` carrier 已从设计中移除**。它原本是"为将来复刻 Electron 无端口模式留位"，但 A.1 证明 Tauri 侧无法经济地承载该协议（真要做得用 `tauri::ipc::Channel` 重写客户端传输层），而 loopback 已被上游论证过安全性。保留一个明知不可行的枚举分支只会误导实现。无端口形态若将来确实需要，应当作为**上游侧**的新 carrier（类似 `dsh-desktop-host`）来提，而不是壳的私事。

### D2 — Node 运行时解析：三级探测链，fail-soft 回退

按用户选择：**两个变体 + 可配置路径优先**。

```
解析顺序（每次冷启动，结果按 (path, mtime, version) 缓存到 app 数据目录）
1. 用户显式配置的 node 路径     ← 设置页 / preset 的可选字段 / 环境变量 DSH_APP_NODE
2. 系统可发现的 node            ← PATH 中的 `node`，以及 /opt/homebrew/bin、/usr/local/bin
3. 内置 runtime/node/node       ← 仅 bundled 变体
```

**探测（doctor）不是只跑 `node -v`**，必须验证三件事（**§11 已按实测修正**）：

1. **版本**落在 `^22.19.0 || >=24.0.0`；
2. **原生模块可加载**：`node-pty` / `koffi` / `node-addon-require-builtin` 能 require（ABI / NAPI 不匹配必须在此暴露，而不是启动到一半崩）；
3. **internals 能力**——**只在 profile 需要时才检查**（见下）。

第 3 条有两处实测得到的修正，写在这里避免实现时踩：

- **"能加载 addon"≠"internals 可用"**。`node-addon-require-builtin` 是通过**匹配运行中 Node 二进制的机器码**来 hook 的，所以它可以 require 成功却拒绝每一个 id。实测 Nix 的 `nodejs-slim` 构建就是如此：`Unsupported/no-getter (arm64 getter is not optional-bti-ldr-x0-this-imm-ret)`，而官方 nodejs.org 构建正常。**doctor 必须真的调用 `requireBuiltin('internal/modules/esm/loader')` 并检查返回值**，否则会给假阳性，让失败推迟到 profile 挂载期（表现为一句误导性的 "`--expose-internals` is required"）。
- **是否需要 internals 取决于 profile 的 `patchReload`**：`live` 才挂 HMR 行、才需要它；我们的默认 `startup` **完全不需要**（已实测：用不支持 addon 的 Nix Node 也能正常启动）。所以 doctor 不应无条件报告 internals 缺失为错误——由 preset 的 `patchReload` 决定它是不是硬性要求。

`--expose-internals` 是**等价的兜底路径**（实测可用）：仅当 profile 需要 internals 且 addon 不支持时由 launcher 加上，并在 UI 提示。不把它作为全局默认——它给 agent 进程更大权限面，而绝大多数配置（`startup`）用不到。

### D2.1 — 第三个运行时选项（记录，不在 v1 实施）

上游已经有一条**单文件可执行**路线：`@yao-pkg/pkg --sea`，把 Node 24 + 闭包打成一个自解压 exe（`scripts/build-exe-for-python-sdk.ts`，产物约 174 MB，已经带 `web` profile 与前端资产，并以平台 wheel 形式分发）。它天然满足"不需要系统 Node、也不需要 app 内摊开 `node_modules`"。

为什么 v1 不做：

1. **签名/公证**：SEA 把 blob 注入 Node Mach-O，公证对这类自改二进制很敏感；bundled 变体走"官方 Node + 展开闭包"是风险更低、上游 Electron 版已验证过的形状。
2. **体积不是瓶颈**：Tauri 壳本身很小，bundled 变体的体积由 Node + 闭包主导，SEA 并不显著更小。
3. **可调试性**：展开闭包出问题时可以直接 `node lib/bin.js` 复现。

保留为 **M9（可选）** 的运行时变体 `bundled-sea`：如果将来要把 macOS 之外的发行也纳入，单文件路线比"每个平台各带一份 Node 归档"更省事。枚举上预留 `RuntimeKind::{Configured, System, BundledNode, BundledSea}`，新增不发散到 carrier 与 UI。

### D3 — Preset：用户可选，但只做"薄封装 + 物化"

用户的直觉是对的：**preset 直接决定子进程怎么起**。设计上把 preset 定义成一次解析的输入，输出就是 `LaunchPlan`：

```yaml
# $DSH_HOME/dsh-app/presets/<id>.yaml   （app 拥有 schema，用户可编辑/复制）
id: work
label: 工作区
kind: web                 # web | task | server   → 决定 carrier 与 UI 语义（见 D3.3）
profile: dsh-app-work     # $DSH_HOME/profiles/<profile>
bundles:                  # 即 profile 的 dsh.profile.bundles
  - '@deepseek-ai/dsh-base'
  - '@deepseek-ai/dsh-web-app'
patches:                  # 额外的 patch 层文件（相对 preset 目录）
  - patch.d/10-company-proxy.yml
patchReload: startup      # 打包默认 startup；dev 变体可显式 live
node: /opt/homebrew/bin/node   # 可选：本 preset 专用的 node
env:                      # 白名单校验后注入子进程
  HTTPS_PROXY: http://...
appArgs: []               # kind=web 时可传 --port 等（默认我们注入 --no-open --port 0）
job: "run the tests"      # 仅 kind=task：位置参数（上游 `dsh --profile headless "<job>"`）
```

解析规则（评审要点）：

- `bundles` 必须**原样落进** `$DSH_HOME/profiles/<profile>/package.json` 的 `dsh.profile.bundles`；app 不解释 bundle 内容。
- profile 首次物化由 app 完成（写 `package.json` + 空 `cordis.patch.yml` + `pnpm-workspace.yaml`，`patchReload` 取 preset 值）。这是对 `initProfile`（`packages/boot/app-boot/src/profile.ts:170-190`）产物结构的复制，**不重写上游语义**；首次物化后该目录归用户所有，app 不再覆盖。
- 环境变量注入走**白名单**（proxy 相关 + 我们自己的 `DSH_*` 子集），并继承 Electron 父进程的经验：剔除 `NODE_OPTIONS`、`npm_*`/`pnpm_*`/`corepack_*`（`apps/desktop/src/host-process.ts:105-110`）。
- **默认 preset 不用上游 `web` 模板的名字**：我们用独立 profile 名（如 `dsh-app`），并在文档里声明"该 profile 由 dsh-app 拥有"，与上游保留 `desktop` 给 Electron 的做法对齐。这样 `dsh --profile dsh-app` 与 GUI 可共享同一份配置与凭据。
- **preset id 与 profile 名都必须全局唯一**（D3.4 的并发前提），加载期校验并拒绝重复。

### D3.1 — Preset 如何映射到"子进程怎么起"（回答：确实是同一个问题的两面）

把 preset 看成**只有一个职责**：产出一份 `LaunchPlan`。它不改 launcher 的语法，只决定喂给 launcher 的入参和载体：

| preset 字段 | 影响的启动要素 | 上游对应物 |
|---|---|---|
| `kind: web` | carrier = Web；窗口加载打印出的 URL | `dsh --profile <p> --no-open --port 0` |
| `kind: task` | 一次性进程，读到退出码为止，输出进日志视图 | `dsh --profile headless "<job>"` |
| `kind: server` | 长期 stdio 进程，仅托管 + 日志，无 UI 附着点 | `dsh --profile acp` / `--profile sdk`（stdio JSON-RPC） |
| `profile` | 决定 `$DSH_HOME/profiles/<p>` 与 `dsh.profile.bundles` 的落点 | `--profile <p>` |
| `bundles` | profile 首次物化时写入 `dsh.profile.bundles`；**不由 app 解释** | `dsh.profile.bundles` 顺序层 |
| `patches` | 追加 patch 层（等价于 `--patch` 或 profile 的 `cordis.patch.yml`） | patch 层栈 |
| `patchReload` | 决定是否给子进程 internals 能力（HMR 行） | `dsh.profile.patchReload` |
| `node` | Runtime Resolver 的**第一优先级** | 无（上游不选运行时） |
| `env` | 白名单注入 | 继承环境 + `.env` 分层 |

关键判断：**preset 不需要新的启动路径**。上游已经把"哪个组合、哪份补丁、哪种运行模式"表达为 `--profile` + patch 层；preset 只是把这份表达**提前到 GUI 里可选、可存、可复制**。如果某个 preset 想表达的语义上游没有对应物（例如"同时跑两个 backend 并聚合"），那就是本仓库要写的代码，而不是能靠配置变出来的——这条边界要在实现时守住，否则 preset 会退化成一个半成品配置语言。

### D3.2 — 并发托管：留缝，但把缝的位置写清楚

不同 preset ↔ 不同 profile，**上游层面确实没有共享写路径**（唯一的写租约是 per-session `flock`，审计 §3）。所以"多开"在 dsh 语义上是安全的。壳这边需要做的是：

- **单实例锁只保护壳**，不禁止多 backend（否则两个壳会抢同一 profile）。
- 壳维护 `Vec<ChildHandle>`，每个 preset 一个子进程、一个随机端口、一份 stderr 环形缓冲。
- UI 用侧栏列出所有 preset 及其状态；`kind: web` 的可以并列成标签页。

**形态取舍（v1 定案）**：

| 阶段 | 形态 |
|---|---|
| **v1** | **单活动 WebView + 后台托管**：任一时刻只有一个 `kind: web` 的 preset 显示在窗口里，但**多个 preset 可以同时运行**（其他 web preset 在后台跑，状态可见、可切换、可单独停止）。`task`/`server` 天然不占窗口。 |
| **v1.1** | 真正的多 WebView 标签页（Tauri 支持多窗口/多 webview；成本主要在 UI 与焦点管理，不在后端）。 |

选这个先后顺序的原因：**"能同时跑"是架构问题，"能同时看"是 UI 问题**。架构上留好缝（句柄模型 + 每个 preset 独立的 carrier 实例 + 状态机），UI 以后再长；反过来做会先把自己锁死在单实例的假设里。

### D3.3 — 没有 UI 的 preset 会发生什么（必须显式定义）

问得对：`kind` 的语义不能只写"决定 carrier"，得说清**谁来消费输出**。这里有一个容易混淆的点：**ACP/SDK 的 stdio 通道是"子进程的 stdio"，不是网络端口**——外部编辑器没办法连上来，上游模型是"编辑器自己去 spawn `dsh`"。所以壳把一个 ACP preset 跑在自己的进程树里，**除了壳自己当客户端之外没有任何消费者**。

三种 kind 的完整语义：

| kind | 上游 profile | 生命周期 | UI | 壳做什么 / 不做什么 |
|---|---|---|---|---|
| **`web`** | `web`（或同构的 base+web-app 组合） | 长期，直到用户停止 | **WebView 加载 runtime 提供的 GUI** | 托管 + 就绪解析 + 归属窗口 |
| **`task`** | `headless` + 位置参数 | **一次性**：跑完就退出 | 日志视图 + 退出码（可复制输出） | 托管 + 收集 stdout/stderr + 呈现结果。**这是"无 GUI 但仍有 GUI 价值"的那一类** |
| **`server`** | `acp` / `sdk` / 自定义 stdio | 长期 | **只有状态与日志**，明确标注"无关联界面" | 只托管：起停、状态、日志尾部、显示可复制的等价命令 |

`server` 类在 v1 的诚实定位：**它不是一个能用的 GUI 功能，而是一个"配置与进程的容器"**。价值有两点：(a) 用户可以在这里维护一份 ACP/SDK 的 patch 组合，然后在编辑器里用 `dsh --profile <name>` 消费同一份 profile；(b) 需要常驻的自动化后端可以在这里起停并看日志。

**明确不做**（避免半成品）：v1 不为 `server` 类提供"附着"UI（不内嵌 JSON-RPC 客户端，也不假装能接管 stdio）。若将来要做，正确形态是壳自己实现 ACP/SDK 客户端并把会话渲染出来——那是一个独立项目，不是一个 preset 开关。

**加载期校验**：`kind` 与其 `bundles` 必须自洽（`web` 必须含 `@deepseek-ai/dsh-web-app`；`task` 必须含 `@deepseek-ai/dsh-headless`；`server` 必须含 acp/sdk 之一），否则**拒绝加载并给出可操作的错误**，而不是起一个永远不出 URL 的进程。这一条直接消灭了"等就绪等到超时"这一类难以诊断的失败。

### D3.4 — 并发托管的四条不变量（竞态清单）

多开不是"没有共享状态"，而是"共享状态很少且已识别"。实现时必须守住：

| # | 共享物 | 风险 | 不变量 |
|---|---|---|---|
| I1 | `$DSH_HOME/profiles/node_modules` 安装回退 | 两个不同 **运行时**（版本）互相改回退 | **一个安装前缀只暴露一个后端版本**（见 D8），因此同一时刻所有 preset 共用同一份回退，不存在跨版本争用。壳启动时断言所有 preset 解析到同一 runtime-id |
| I2 | `$DSH_HOME/.credentials.yaml`（0600，读一次/启动） | 两个 preset 首次启动同时创建/写 | 壳串行化子进程启动（一次只 spawn 一个 preset 并等它就绪），把这个窗口从"并发"降为"不存在"；这是**唯一需要串行化的点** |
| I3 | 插件安装（`dsh plugin` / pnpm store） | 两个 preset 同时装插件写同一 store | v1 规则：**插件操作全局串行**（壳内一把互斥锁），且操作期间禁止启动新的 preset；UI 明确显示"正在安装插件"。pnpm 自身的 store 并发是安全的，但 `$DSH_HOME/profiles/<p>/package.json` 的 reconcile 不是 |
| I4 | 崩溃与重启 | 一个 preset 崩溃重启影响其他 preset 的稳定性 | 每个 preset 独立的重启预算与退避；**同一页面共享的 `lastGood` 回退只作用于版本层面**，不因单个 preset 的崩溃而回退整个运行时 |

另外两条边界：

- **运行时切换的前提**：切换后端版本时，如果还有 preset 在运行，壳必须**全部停止 → 切换 → 由用户重新启动**（不静默重启，因为不同版本的会话不可混用）。UI 要说清这一点。
- **壳退出时的清理顺序**：停止全部 preset → 释放单实例锁。上游 profile 没有锁，所以壳退出后 `dsh --profile <p>` 从 CLI 立即可用（这是刻意的：preset 不该把 CLI 锁在门外）。

### D4 — 打包版 profile 默认 `patchReload: startup`

`web` 模板是 `live`（`profile.ts:115-118`），会挂 HMR 行并在无 internals 访问时**抛错**。打包应用的默认 preset 用 `startup`：

- 免掉对 internals 能力的硬依赖（更小的失败面）；
- 生产环境不需要 HMR；
- 保留 `dev` preset（`patchReload: live`），供本仓库开发与高级用户使用，并在 UI 标注代价。

### D5 — 进程与生命周期

- **单实例**：`tauri-plugin-single-instance`。上游**没有** home/profile 级锁（审计 §3），壳必须自己提供，否则两个壳会争同一 profile 的 `sessions/**` 写租约。注意这把锁**只锁壳**，不限制壳内并发托管多个 preset（D3.2）。
- **启动**：`spawn(node, [entry, '--profile', profile, '--no-open', '--port', '0'], { cwd: project, env, process_group(0) })`。`--no-open` 必须给，否则 dsh 会额外拉起用户默认浏览器（`packages/bundle/web-app/src/index.ts:273-279`）。
- **就绪**：读 stdout，按 `dsh web: ` 前缀抓 URL（含 token），设 30s 超时；超时则收集 stderr 尾部并报错。
- **退出**：`SIGTERM` → 等 5s（上游 `PROCESS_SHUTDOWN_TIMEOUT_MS = 5_000`，`apps/cli/src/process-shutdown.ts:4`）→ `SIGKILL`；窗口关闭即触发；父进程被杀时靠进程组 + 子进程自身对 stdin 断开的处理兜底。
- **崩溃恢复**：子进程非零退出 → UI 显示诊断（stderr 尾部）+ 重启按钮；同一 preset 连续崩溃 3 次则停止自动重启（**预算是 per preset 的**，D3.4 I4）。
- **退出顺序**（D3.4）：停止全部 preset（并发发 SIGTERM，再并发等 5s）→ 确认无残留 → 释放单实例锁。上游 profile 没有锁，所以壳退出后 CLI 立即可用。
- **窗口**：所有导航限制在 `http://127.0.0.1:<port>` 与 `about:blank`；外链走 `tauri-plugin-opener` 交系统浏览器。

### D6 — 壳与运行时的契约：**只有一处**，而且我把它降级成了可选项

上游 Electron 是**点锁步**：壳、dsh、seed、Node、pnpm 一个版本，dsh 升级就是一次 Desktop 发布（`apps/desktop/README.md` 的 Release identity 行），理由是"独立版本会产生未经测试的组合"。

本设计里壳对运行时的读取面只有四件事：

1. 以指定 `cwd` + 白名单 `env` 启动 `node <entry> --profile <p> --no-open --port 0`；
2. 从 stdout 读 `dsh web: <url>` 这一行；
3. `SIGTERM` → 5s → `SIGKILL`，并清理进程组；
4. 把 WebView 指到该 URL，导航限制在 loopback。

这四件事与 dsh 的插件图、客户端模块表、`__DSH_BOOT__` 结构**完全无关**——这也是选 loopback carrier（D1）的另一个理由。**所以我不打算为它引入一个版本号协议**：一个只被读一行的契约，"约定版本"带来的收益比它的维护成本小。

代价是：**"我读到的那一行会不会变"没有编译期保证**。处置不是加版本号，而是三条实证手段：

- 解析用宽松正则（只要求 `dsh web: ` 前缀 + 一个可解析的 loopback URL），并对这一行单独写单测；
- **安装后强制冒烟**（D8.2 U1）——这是真正证明"这一版运行时能被这个壳带起来"的检查；
- 版本清单里保留 `dshVersion` / `commit` 字段，出问题时能立刻定位是哪一个组合。

保留 `runtime.json`（下节），但它的 `shellContract` 字段只作为**声明性的卫生检查**（值不匹配时给出可读错误而非静默继续），不作为门禁。

### D7 — 运行时布局：**一次安装一个真实版本**（大幅简化）

这是本轮最重要的简化。上一版为"多版本并存 + 任意切换"设计了运行时区和跨版本 home；**收敛后的版本策略（D8）让这整套东西都不需要了**。

仍然成立的上游硬约束（必须尊重，但不再需要复杂方案）：

> `$DSH_HOME/profiles/node_modules` 是**全 home 共享的单一安装回退**，镜像"本次启动的 dsh 安装"的依赖闭包，每次启动自愈（`packages/boot/app-boot/src/profile.ts:19-21, 469-497, 541-565`）。因此**同一个 `DSH_HOME` 下不能同时跑两个不同 dsh 安装**。

因为一次安装只有一个后端版本，这条约束**自动满足**：所有 preset 共享同一份回退，不存在跨版本争用（D3.4 I1）。

**布局**：

```
$DSH_HOME/                                 （默认 ~/.dsh；壳与 CLI 共用，子进程的 DSH_HOME 就是它）
├── .credentials.yaml                      ← 共享的凭据（GUI 与 CLI 同一份）
├── settings.yaml、sessions/、storages/…    ← 共享的产品数据
├── profiles/…                             ← CLI 的 profile 与 GUI 的 preset profile 并存
└── dsh-app/                               ← 只属于 dsh-app；卸载时唯一可以删的子树
    ├── runtimes/
    │   ├── current -> 0.1.5-rc.1/         ← 原子替换的符号链接 = 活动版本
    │   ├── prev/                          ← 上一次成功激活的版本（唯一回滚点，D8.2 U4）
    │   └── 0.1.5-rc.1/
    │       ├── runtime.json
    │       ├── node_modules/@deepseek-ai/dsh/…
    │       └── node -> ../../node/darwin-arm64/24.17.0/bin/node
    ├── node/darwin-arm64/<ver>/           ← Node 安装（更新时复用，不重复下载）
    ├── pnpm/                              ← 内置 pnpm（插件管理用）
    ├── presets/<id>.yaml                  ← 用户 preset（D3）
    └── state.json                         ← 通道、当前版本、上次成功启动、各 preset 崩溃计数
```

**与上一版的关键差别**：

| 上一版（多版本并存） | 现在（一次安装一版本） |
|---|---|
| `runtimes/<channel>-<ver>/` 多份并存 | `runtimes/current` 单份 + `prev/` 回滚点 |
| 每个运行时一个 `homes/<runtime-id>/` | **单一 `$DSH_HOME`**，所有 preset 共享，CLI 也用这一个 |
| 跨版本不共享会话/凭据（要写导入向导） | **天然共享**：切版本后会话、设置、凭据都在（Q4 消失） |
| 需要运行时选择 UI + 一致性校验 | 不需要：只有一个后端版本 |

**一个 home 是更好的产品语义**：用户视角里"我的会话和配置"属于这个产品，不属于某个后端版本，也不属于某一个前端。上游 Electron 之所以要独立 home，是因为它要和**同时存在的 CLI** 隔离可执行包；我们和 CLI 的边界同样是"可执行包各在自己的安装里、产品数据在同一个 home"，但因为我们不再需要多版本并存，所以不需要为它再切一层目录。

**与 CLI 的关系**：打包态的 home 就是 `~/.dsh`，所以 `dsh --profile <name>` 与 GUI 的 preset 看到的是同一份会话、设置和凭据——登录一次两边都可用，这是"同一个产品的两个前端"应有的行为（Q5）。代价是 GUI 与 CLI 可能各带一个 dsh 安装对着同一个 `$DSH_HOME/profiles/node_modules` 回退，也就是上面那条上游约束；0.1.6 起 CLI 默认走进程内 runtime 解析，不再每次启动改写那张符号链接表，互剪的根因因此消失。需要完全隔离时用 `DSH_APP_HOME=$(mktemp -d)` 起一个独立的壳。

**Node 复用**：原生模块只按 `os`/`cpu` 分发（Node-API/NAPI，不按 Node 主版本；`@deepseek-ai/node-addon-system` 的 optionalDependencies 只有 `-darwin-arm64` / `-darwin-x64` / `-linux-*`），所以更新 dsh 版本时**通常不需要换 Node**。`node/darwin-arm64/<ver>/` 支持并存多个 Node 版本，`node ->` 链接按清单选择；更新时若新运行时的 `node.requirement` 不满足当前 Node，才下载第二份。

### D8 — 版本策略（定案）：一次安装 = 一个真实版本；只有"通道追新"，没有任意版本挑选

**决定**：不在 app 里做任意版本切换/动态安装。一次安装只承载**一个真实的后端版本**，用户能做的只有两件事：

1. **master 通道 → 更新到最新 master**（跟随 harness `main` 的 CI 产物）；
2. **release 通道 → 更新到最新 release tag**（跟随 npm 的发布）。

这比 D6/D7 的"多版本并存 + 运行时区"简单得多，也把上一版最贵的那部分（运行时区、跨版本 home、版本选择 UI）**整个删掉了**。理由：

- **任意版本组合是上游明确要避免的东西**：Electron 那份设计文档把"壳/客户端/后端/插件图是一个被验证过的组合"写成 Release identity，独立版本会造出未测试的组合。用户自己挑一个三个月前的版本，等价于手工制造这种组合。
- **"最新"这件事可以在不引入版本矩阵的前提下满足**：想追新的用户要的是"别让我手动下载 dmg"，而不是"让我挑版本"。这个诉求由 updater 满足。
- **回滚仍然有**：不是"选任意版本"，而是"回到上一个已安装的版本"（下面 U4）。

### D8.1 — 通道模型（收敛后的 `channel.rs` 职责）

| 通道 | 来源 | 版本形态 | 更新动作 |
|---|---|---|---|
| **release** | npm registry `@deepseek-ai/dsh@latest` | `0.1.5-rc.1` | "检查更新" → 有则提示 → 用户确认 → 更新到最新 tag |
| **master** | 本仓库 CI 从 harness `main` 构建的产物 | `0.1.6-dev.<short-sha>.<yyyymmdd>` | 同上，但目标是最新 master 产物 |

通道在**安装时**由 cask 决定（D9）：`dsh-app` = release，`dsh-app@master` = master。**v1 不再支持"装完再换通道"**——换通道 = 换 cask（`brew reinstall --cask`）。这消掉了通道切换器、以及随之而来的"两个通道并存于一个 home"的全部复杂度。

`channel.rs` 因此只剩三个操作：`current()`（读清单里的通道与版本）、`check()`（查该通道有没有更新的目标）、`update(target)`（下载 → 校验 → 装到准备目录 → 冒烟 → 原子替换）。

### D8.2 — 更新流程（唯一需要谨慎的写路径）

```
check()     查 npm dist-tag / 本仓库 release → 得到 target 版本 + sha256
update()    1. 下载到 runtimes/.staging/<ver>/
            2. 校验 sha256 + runtime.json 的 shellContract
            3. 解析依赖闭包（原生包按 os/cpu 落地）
            4. 【冒烟】起一次子进程 → 等到 "dsh web:" URL 行 → 立即停止
            5. 全部 preset 停止（若有在跑的，UI 先征求确认）
            6. 原子替换：runtimes/current → <ver>；旧版本移到 runtimes/prev/
            7. 重新签名（若安装位置在 app bundle 内，见下）
```

**U1 冒烟是强制的**：这是"点锁步降级为区间锁步"（D6）之后唯一的验证补偿。冒烟不过就丢弃 staging，保留现状，并报告 stderr 尾部。

**U2 更新写在哪里**：运行时**不在 app bundle 内**（bundle 只读、且改动即破坏代码签名）。运行时与状态都在 `$DSH_HOME/dsh-app/`，更新只是用户目录内的目录替换。这也意味着**更新不需要重新签名 app**——签名只覆盖壳与预装内容。

**U3 预装的 vs 更新后的**：app bundle 里的 `Resources/runtimes/<...>` 是**首次启动的种子**（保证离线可用），`$DSH_HOME/dsh-app/runtimes/` 是**运行时实际来源**。首次启动做一次 seed → home 的复制，之后只从 home 读。这与上游 Electron 的 seed 模型同构，但更简单：没有 pnpm store 分片，因为闭包是展开的。

**U4 回滚 = 回到 `prev/`**：保留上一次成功激活的版本，UI 提供"回到上一版本"（对 master 通道尤其必要，因为 master 未经发布验证）。再往前的版本不保留——**这就是"不做任意版本切换"的具体含义**。

**U5 master 的旧版本清理**：`prev/` 只留一份；更早的 master 版本直接删除（它们没有长期价值，且 master 版本没有"某个特定版本很重要"的语义）。

**U6 与多 preset 并发的顺序**（呼应 D3.4）：更新时若有 preset 在跑，必须先全部停止再替换，替换完成后**不自动重启**（版本变了，用户的会话上下文也变了，应由用户决定何时重开）。

### D9 — Homebrew：通道在 install 时决定

两个 cask，**共用同一个 `.app`**，差异只在预装哪个通道的种子运行时：

| cask | 预装 | 后续更新目标 |
|---|---|---|
| `dsh-app`（默认） | 打包时的最新 release | 最新 release tag |
| `dsh-app@master` | 打包时的 master 产物 | 最新 master |

这与 D8 的"通道由装决定"完全对齐：**brew 负责通道身份，app 负责在该通道内追新**。也顺带解决了上一版的一个矛盾——如果 app 内能自由换通道，cask 的 `version` 字段就不再代表任何稳定含义；现在 cask 的 `version` 就是"预装运行时的版本"，语义清晰。

注意 mac 上 cask 的 Node 依赖问题（§6.1 表格里的 formula 行）与这里无关：bundled 变体的运行时自带/共享 Node。

### D10 — 更新模型定案：B 为主 + A 兜底（运行时独立更新，壳走 cask）

用户确认的取舍。**两个部件各有各的更新通道：**

| 部件 | 变更频率 | 更新路径 | 是否需要重新公证 |
|---|---|---|---|
| **运行时**（dsh 闭包 + Node） | 跟随 dsh 发版，高 | **B**：`$DSH_HOME/dsh-app/runtimes/` 内的原子替换 + 强制冒烟（D8.2/M5） | 否 |
| **壳**（Tauri app bundle） | 只在壳自身代码改动时 | **A**：`brew upgrade --cask` 换成新的签名 dmg（M9） | 是 |

**为什么 B 为主**：壳极薄（一个状态页 + 进程管理），其变更频率远低于 dsh；把两者绑成"一次 dsh 发版 = 一次 Apple 签名公证 + 用户下整个 dmg"（上游 Electron 的选择）会让发行流程被后端版本绑架。

**为什么留 A 兜底**：壳要改时本来就要重新公证，走 cask 是自然路径；而且它是唯一能替换壳的机制。

**明确放弃的东西**（必须写下来，不能含糊）：

1. **运行时不在签名 bundle 内**。它在用户 home、由用户自己的 app 下载、无 quarantine 标记，因此当前不触发 Gatekeeper——但这确实**绕开了 macOS 的代码校验模型**。
2. **不能上 Mac App Store**。沙箱禁止这种"bundle 外运行下载代码"的形态；若将来要上架，只能退回纯 A。
3. **`prev/` 只能回滚运行时，回滚不了壳**。壳更新出问题时用户得 `brew install` 指定旧版的 cask。
4. **失去了 bundle 级的 OS 验证机制**，校验只能自己做（sha256 + 冒烟），所以那两步是**强制**的，不是可选优化。

**两条对既有决定的修正：**

- **cask 的 `version` 改为壳的版本**，不再是"预装运行时的版本"。理由：运行时可以独立更新，把它写进 cask 版本会让 `brew` 的心智模型和 app 内状态立刻不一致（装的是 0.1.5，app 里已经自更新到 0.1.6）。预装运行时版本改为记录在 `Resources` 里的一个字段，供"种子是什么"的诊断使用。→ 修正 §6.1。
- **M6 的 CI 产物形态**：master 通道发的是**运行时归档**（配合 B），不是整个 dmg；整个 dmg 只在壳变更时由 `app-release` 产出。
---

## 4. 目录结构

```
dsh-app/
├── flake.nix                     # devShell + packages.{dsh-runtime,dsh-app-bundled,dsh-app-slim,app}
├── flake.lock
├── README.md
├── docs/
│   └── design-tauri-wrapper.md   # 本文
├── package.json                  # pnpm workspace 根；@tauri-apps/cli 等
├── pnpm-workspace.yaml
├── src-tauri/                    # 标准 Tauri 布局
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   ├── icons/
│   ├── build.rs
│   └── src/
│       ├── main.rs
│       ├── preset.rs             # 定义 + 解析 + 物化 profile + kind/bundles 自洽校验（D3）
│       ├── runtime.rs            # Node 三级探测 + doctor（D2）
│       ├── runtime_store.rs      # runtimes/current|prev 的激活、冒烟、回滚（D7/D8.2）
│       ├── channel.rs            # release(npm latest) / master(CI 产物) 的 check + update（D8.1）
│       ├── manifest.rs           # runtime.json schema（声明性卫生检查，D6）
│       ├── launch.rs             # LaunchPlan 构造
│       ├── supervisor.rs         # 多 preset 句柄表、状态机、崩溃预算、并发不变量（D3.2/D3.4）
│       ├── carrier/
│       │   ├── mod.rs            # trait Carrier
│       │   ├── web.rs            # WebLoopback（kind=web）
│       │   ├── task.rs           # 一次性进程（kind=task）
│       │   └── server.rs         # 长期 stdio，仅托管 + 日志（kind=server）
│       ├── lifecycle.rs          # 单实例锁 / 信号 / 退出编排 / 清理顺序
│       └── commands.rs           # 前端 IPC：preset 列表与状态、启停、更新、日志、回滚
├── ui/                           # 极薄壳页面（启动画面 / 错误 / preset 选择）
│   └── index.html + src/
├── scripts/
│   ├── prepare-runtime.mjs       # 下载/校验 Node → runtime/node（bundled）
│   ├── prepare-dsh-closure.mjs   # pnpm deploy --legacy --prod → runtime/dsh
│   ├── sync-version.mjs          # dsh 版本 → tauri.conf.json / Cargo.toml
│   └── package-macos.mjs         # tauri build + 签名 + 公证 + dmg + cask 生成
├── nix/
│   ├── dsh-runtime.nix           # npm 闭包（FOD）
│   ├── app-bundle.nix            # 手搓 .app 布局（纯 Nix，macOS）
│   └── node-runtime.nix          # 固定版本 Node 归档（FOD, 带 sha256）
├── packaging/
│   ├── homebrew/Casks/dsh-app.rb.tmpl
│   └── entitlements.plist
└── .github/workflows/release.yml # macOS runner：构建 → 签名 → 公证 → 上传 + 更新 cask
```

`src-tauri/` 放在仓库根是 Tauri 的约定；本仓库目前没有 `apps/` 层，保持单应用布局更省事。若将来要加第二个二进制（例如 CLI 伴侣），再引入 workspace。

---

## 5. Nix 集成

### 5.1 devShell（对现有 flake 的最小改动）

现有 `flake.nix` 已组合 `ts.base + ts.rust.stable + ts.nodejs."22"`。需要补：

```nix
{
  packages = with pkgs; [
    pkg-config
    # macOS 无需额外 framework：Tauri/wry 链接系统 WKWebView
  ] ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
    pkgs.apple-sdk_14
  ];
  env = {
    DSH_APP_DEV = "1";
  };
  shellHook = ''
    export PATH=$PATH:./node_modules/.bin
  '';
}
```

两处细节：

- **判平台用 `pkgs.stdenv.hostPlatform.isDarwin`**（本稿早前写成 `pkgs.st2.stdenv.isDarwin`，笔误，已改）。
- **不要用 `pkgs.darwin.apple_sdk.frameworks.*`**：本仓库 pin 的 nixpkgs（`3aadb7ca`，2026-02）里 `darwin.apple_sdk` 已作为 legacy stub 移除，直接用 `pkgs.apple-sdk` / `apple-sdk_14` / `apple-sdk_15`（该 revision 上分别求值为 `apple-sdk-14.4` / `-14.4` / `-15.5`）。

> 注意：`ts.rust.stable` 来自 `acehinnnqru/devshells`，用 fenix 的 stable toolchain。**devshells 目前没有任何 Tauri 相关 toolset**（已核对 `toolsets/` 与 `langs/`）。可考虑回上游给 `devshells` 加一个 `toolsets/tauri.nix`（Tauri CLI 由 npm 提供，Nix 侧只需系统的 WebKit/framework 与 `cargo-tauri` 可选），但这是独立改动，不阻塞本仓库。

### 5.2 构建产物

| attr | 内容 | 可复现性 |
|---|---|---|
| `.#dsh-runtime` | npm 闭包（`package.json` + `node_modules`），由 `pnpm deploy --legacy --prod` 产出 | 纯 Nix（`fetchPnpmDeps` / `pnpmDeps` FOD + 离线构建） |
| `.#node-runtime` | 固定版本官方 Node 归档，解包后放 `Resources/node/<platform>/<ver>` | FOD（`fetchurl` + sha256），与 Electron 的 `prepare-runtime.ts` 同策略 |
| `.#dsh-app-slim` | 壳 + 预装运行时闭包（无 Node） | 纯 Nix（`cargo-tauri.hook`） |
| `.#dsh-app-bundled` | 上者 + `node-runtime` | 纯 Nix |
| `.#dmg` | 由 `.app` + `hdiutil` 生成（仅 macOS；headless 下需 `CI=true`） | 半纯（sandbox 内 `hdiutil` 未验证，A.7） |

**Nix 路线（按调研结论收敛）**：

- 用 nixpkgs 官方的 **`cargo-tauri.hook`**（A.5）。**没有** `tauriBundle`/`buildTauriApp` 这类 helper，不要去找。
- 它默认只出 `app` bundle（Darwin），DMG 另做。
- 需要 pnpm 属性和 `fetchPnpmDeps`/`pnpmConfigHook` → 依赖 `nixpkgs-unstable`（本仓库 flake 已如此），25.05 上没有这些属性。
- **`beforeBuildCommand` 由 tauri-cli 执行，不在 cargo 链路上**：Nix 里被接管的是 cargo（`buildRustPackage` / `cargo-tauri.hook`），而 hook 由 CLI 的 `helpers::run_hook` 在 `dirs.frontend` 下起子进程。要在里面跑 `pnpm install && vite build`，就得给它网络或预取 store、可写缓存与 `HOME`，等于把不确定性塞进本该确定的那一层。**本设计里这个 hook 是空的，所以不执行**（CLI 里是 `if let Some(before_build)`，空值直接跳过）。

**占位前端（这是 Nix 可复现性的前提）**：`ui/` 只是仓库里的手写静态文件（启动画面 / 错误页 / preset 列表），`frontendDist` 直接写相对路径，`beforeBuildCommand` 留空。Tauri 构建因此**完全不依赖 harness 源码、也不需要 pnpm 或 vite**。

`frontendDist` 为什么"必须存在"（已核实，两处检查、两个层）：

1. **CLI 层**（`crates/tauri-cli/src/build.rs`）：hook 跑完后立刻 `if !web_asset_path.exists() { bail!("Unable to find your web assets…") }`，同一处还拒绝把 `frontendDist` 指成 `src-tauri` 目录本身；
2. **编译期**（`crates/tauri-codegen/src/context.rs`，`generate_context!()` 宏展开时）：`FrontendDist::Directory(path)` 分支先 `panic!` 若路径不存在，然后 **`EmbeddedAssets::new(assets_path, …)` 递归读取整棵目录树嵌入二进制**。

所以它不是"构建前的一个准备步骤"，而**就是 Rust 构建（过程宏执行）的一部分**——这是"必须在 cargo 之前存在"的确切含义。相关的边界：`EmbeddedAssets` 会**递归读**该目录，因此它必须可读；Nix store 只读不影响（我们只读不写），但若将来有工具想在构建期往 `frontendDist` 里写东西，在 Nix 下会失败。

**关键边界**：dsh 真正的 Web GUI **不在这个约束范围内**——它由子进程在运行时经 HTTP 提供，不经过 `generate_context!()`。Tauri 只嵌那三个静态页面。这正是"壳的构建不依赖 harness 源码"的根因。

若将来要在 Nix 里跑官方 bundle（前端来自另一个 derivation），做法是 `postPatch` 剥掉 hook + 用 `--config`/`TAURI_CONFIG` 把 `frontendDist` 指向 store 里的预构建目录。**当前设计用不上这条路。**

**`externalBin` 与 Resources 的分工**（由 A.3/A.4 决定）：

- 壳需要的**可执行**（bundled 变体的 `node`）放 `Contents/MacOS/` 或作为 `externalBin`，因为 sidecar 解析基准是 `current_exe().parent()` 且 Tauri 只签 `MacOS` 等六个目录；
- 纯**数据**（运行时闭包 `node_modules`、`ui/`、预装 `runtime.json`）放 `Contents/Resources`，由我们自己的签名脚本处理其中的 Mach-O。

### 5.3 为什么 Nix 仍然是"打包运行"的主干

即使用户最终通过 brew 安装，**Nix 仍是权威构建定义**：CI 与本地都跑 `nix build .#dsh-app-bundled` 得到可签名的 `.app`，brew cask 只是把签名后的产物分发给不装 Nix 的用户。这样"能复现"与"好安装"两件事不必二选一。

**但有一处必须诚实**：签名与公证**不能放在 Nix 里**（需要钥匙串访问、网络、时间戳服务，且 Nix 的产物应当是确定性的）。正确分工是：

```
nix build .#dsh-app-bundled  →  .app（未签名、确定性的）
        ↓ 交给 scripts/sign-and-notarize.mjs（在 Nix 之外，需要凭据）
已签名 + 已 staple 的 .app  →  dmg  →  cask
```

这正好利用 A.4 的结论：签名清单是我们自己穷举的，所以"Nix 出未签名产物 + 脚本逐文件签"比"在 Nix 里跑官方 bundle"更可控。

---

## 6. Homebrew 分发

### 6.1 两条轨道

| 轨道 | 命令 | 适用 | 依赖 |
|---|---|---|---|
| **cask：release（默认）** | `brew install --cask acehinnnqru/tap/dsh-app` | 绝大多数用户 | 无（bundled 变体自带 Node） |
| **cask：master** | `brew install --cask acehinnnqru/tap/dsh-app@master` | 想跟 harness `main` 的用户 | 无 |
| **formula（可选）** | `brew install acehinnnqru/tap/dsh-app-slim` | 想要 slim / 已有 Node 的用户 | `depends_on "node@22"` 或 `node` |

两个 cask **共用同一个 `.app`**，差异只在打包时预装的运行时（D9）：release cask 预装 npm 上的 release 运行时，master cask 预装 CI 产出的 master 运行时。因此 app 内的通道切换器对两者都可用，只是初始状态不同。

cask 模板由 `scripts/package-macos.mjs` 生成（含 arm64/x64 两个 URL 与 sha256），推送到 tap 仓库：

```ruby
cask "dsh-app" do
  version "0.1.0"                # 壳的版本（见 D10：运行时独立更新，不写进 cask 版本）
  sha256 arm:   "<sha256>", intel: "<sha256>"
  url "https://github.com/acehinnnqru/dsh-app/releases/download/v#{version}/dsh-app-#{version}-#{arch}.dmg"
  name "DeepSeek Harness"
  app "dsh-app.app"
  # 只删壳自己的子树；~/.dsh 本身是用户的 harness home，里面有会话，绝不能整棵删
  zap trash: ["~/.dsh/dsh-app"]
end
```

**版本号语义（D10 修正）**：cask 的 `version` 是**壳的版本**。曾经打算让它表示"预装运行时的版本"，但运行时既然能独立更新，那个语义会在用户第一次自更新后立刻失效。预装运行时的版本改为记录在 `Resources` 里供诊断，app 的"关于"分别显示壳版本与当前运行时版本。

master cask 的 `version` 仍用 `0.1.6-dev.<sha>.<date>` 这类形态（它预装的是 master 运行时种子），但必须自定义 `livecheck`，否则 brew 会拿它去和 npm 的 release 版本比较。

**诚实提示**：slim 变体在 **cask 里无法表达 Node 依赖**（cask 的 `depends_on formula:` 不能保证 GUI app 的 PATH）。因此 slim 走 formula，且 formula 里需要写一个 wrapper 把 `node` 的绝对路径固定下来，或让 app 的 doctor 自己去 brew 前缀找 `node`（我们的三级探测已经会看 `/opt/homebrew/bin`，两条都留着）。

### 6.2 CLI 的边界（本阶段不做）

`brew install dsh` 级别的 CLI formula 属于另一个发行物，本阶段**不做**；但设计上不冲突：GUI 与 CLI 共享 `$DSH_HOME` 的产品数据，各自拥有可执行包。上游 Electron 版就是这么划的（`apps/desktop/README.md` 的 State ownership 行）。

---

## 7. 构建与发布流水线

### 7.1 三个阶段

```
阶段 1  runtime closure（平台无关的 npm 部分，可在任一平台产出）
        pnpm add @deepseek-ai/dsh@<pinned>
        pnpm deploy --legacy --prod --config.node-linker=hoisted \
             --config.auto-install-peers=false --config.link-workspace-packages=true \
             <stage>
        node scripts/verify-runtime-closure.ts   # 复用上游校验思路
        → runtime/dsh/{package.json,node_modules/**}

阶段 2  node runtime（仅 bundled）
        下载 node-v<ver>-darwin-<arch>.tar.gz → sha256 校验 → runtime/node/node

阶段 3  macOS 打包（必须在 macOS runner）
        cargo build --release（或 tauri build --bundles app）
        组装 .app + Resources
        codesign --deep 逐个嵌套 Mach-O（顺序：.node → node/dist 二进制 → app）
        codesign --options runtime --entitlements packaging/entitlements.plist
        xcrun notarytool submit --wait
        xcrun stapler staple
        hdiutil create → dmg；生成 cask；上传 release
```

### 7.2 签名/公证的硬骨头（**流水线的核心步骤，先排期**）

两个来源同时指向同一结论：上游为**同样的**问题写了一套 `macos-seed-store.ts`（在 pnpm CAS 里逐个重签 Mach-O 并重写索引哈希）；而 **Tauri 的 bundler 永远不会签 `Contents/Resources`，并且按扩展名跳过 `.node`**（附录 A.4）。所以：

#### 需要提供的凭据（命名对齐上游，便于共用 secret）

> **时机：M8 才需要。** M0–M7 全部用**未签名**产物开发与验证（本地双击会被 Gatekeeper 拦，右键"打开"即可）。凭据的具体获取位置在进入 M8 时再补全（TODO）。

| 变量 | 内容 | 来源 |
|---|---|---|
| `DSH_APP_SIGNING_IDENTITY` | 证书名，**去掉 `Developer ID Application: ` 前缀** | TODO（M8 前补） |
| `DSH_APP_TEAM_ID` | 10 位 Team ID | TODO |
| `APPLE_API_KEY` | `.p8` 私钥的**绝对路径** | TODO |
| `APPLE_API_KEY_ID` | Key ID | TODO |
| `APPLE_API_ISSUER` | Issuer UUID | TODO |
| `DSH_APP_APP_ID` | reverse-DNS bundle identifier | 自定 |
| `MACOS_CERT_P12` / `CERT_PW` | 证书导出（含私钥）+ 导出密码，仅 CI 用 | 钥匙串导出 |

前置：**Apple Developer Program 会员**（Developer ID Application 证书只有付费账号可签）。
不推荐 Apple ID + app-specific password 那条替代路径（上游也支持），API key 不会因改密码失效、权限可收紧。

#### 需要签名/公证的产物

| 产物 | 处理 |
|---|---|
| `*.node`（`node-pty`/`koffi`/`node-addon-require-builtin-*`/`system.node`） | 逐文件重签（**无须** entitlements，除非它自己要 JIT） |
| `@vscode/ripgrep/bin/rg`、`node-pty/**/spawn-helper`、任何 `.dylib` | 逐文件重签 |
| `Contents/MacOS/node` | 重签**并带 entitlements**（见下） |
| `.app` | `--options runtime --timestamp --entitlements packaging/entitlements.plist` |
| `.dmg` | 单独签 + 单独公证 |

**不要期待 Tauri 帮你签任何东西（除了 `MacOS/` 等六个目录），签名清单必须由我们穷举**：

- `Contents/MacOS/node`（bundled 变体；放 MacOS 是因为 sidecar 解析基准，A.3）
- `Contents/Resources/**/*.node`：`node-pty`、`koffi`、`node-addon-require-builtin-*`、`@deepseek-ai/node-addon-system/bin/system.node`
- `node-pty/**/spawn-helper`、`@vscode/ripgrep/bin/rg`
- 任何 `.dylib` / 可执行脚本

做法与断言：

1. 逐文件 `codesign --force --options runtime --timestamp -s <Developer ID>`（**不用 `--deep`**：已废弃且到不了 Resources）。
2. **`Contents/MacOS/node` 必须带 entitlements 重签**，见下（这是最容易翻车的一处）。
3. 断言：`codesign --display --verbose=4` 含 `Authority=Developer ID Application: …`、`TeamIdentifier=…`、`Timestamp=`、`flags=0x…(runtime)`（字段名取自上游 `verify-macos-signature.mjs` 的断言）；再 `codesign --verify --deep --strict` + 冷启动冒烟（真实 doctor + 真起一次子进程 + 等到 URL 行）。
4. "签名后不得再修改任何被签文件"要写成流水线检查（例如签名前后 `find -exec shasum` 对比）。
5. `xcrun notarytool submit --wait` → `xcrun stapler staple` → 再冒烟一次。**dmg 要单独签、单独公证**（上游 `notarize-macos-disk-images.mjs` 就是这么做的）。

#### Node 的 entitlements（已核实官方二进制）

`codesign -d --entitlements - node-v24.17.0-darwin-arm64/bin/node` 显示官方 Node 自带六项：

```
com.apple.security.cs.allow-dyld-environment-variables     true
com.apple.security.cs.allow-jit                            true
com.apple.security.cs.allow-unsigned-executable-memory     true
com.apple.security.cs.disable-executable-page-protection   true
com.apple.security.cs.disable-library-validation           true
com.apple.security.get-task-allow                          true   ← 必须去掉
```

三条结论：

- **重签会剥掉 entitlements**，而 V8 的 JIT 在 hardened runtime 下需要 `allow-jit` + `allow-unsigned-executable-memory` → 必须显式给（`--entitlements`），否则 Node 起不来。**不要用 `--preserve-metadata=entitlements` 图省事**，原因见下条。
- **`get-task-allow` 必须去掉**：它允许调试器附着，是开发签名的标志，**公证会拒绝**。所以 `packaging/node.entitlements` 里只写前五项。
- **[改] `disable-library-validation` 需要保留**（本稿早前写"全部同 Team ID 重签所以不需要"，不准确）：官方 Node 就是带它发布的，本来就预期加载外来的 `.node`。我们原样保留它，而不是自己决定加不加。

#### CI 里的证书

`.p12`（含私钥）→ base64 → secret，导入后**必须**设 partition list，否则 `codesign` 会弹钥匙串授权框，在 CI 里表现为静默卡死：

```bash
security create-keychain -p "$TMP_PW" build.keychain
security import cert.p12 -k build.keychain -P "$CERT_PW" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -k "$TMP_PW" build.keychain
security list-keychains -d user -s build.keychain
```

#### 已知的 spawn-helper 风险

`node-pty` 的 `spawn-helper` 走 `posix_spawn`，在"被签名 + hardened runtime"之后有已知失败案例（上游 `@deepseek-ai/dsh-subprocess-local` 的 postinstall 专门 `chmod` 它就是这个原因）。**M8 的冒烟必须包含一次真实 PTY 相关操作**，不能只验证 GUI 起得来。

**已知风险**：`externalBin` 的公证有未解决的失败报告（tauri#11992，主二进制签名无效）。因此 **M8 的第一件事是端到端试签**，在两条路线里选：

- **路线 1（本稿默认）**：`cargo build`（Nix 出未签名 `.app`）→ 我们的脚本逐文件签名 → `hdiutil` 出 dmg。确定性最高，且完全绕开 tauri#11992。
- **路线 2**：用 `cargo-tauri` 官方 bundle（自动放 `externalBin` 并签 `MacOS` 目录），把 Resources 的签名挂在它之后，接受 tauri#11992 的不确定性。

**为什么这条决定 Node 策略**：bundled 变体把"要不要处理这套签名清单"变成一次性的构建期工作；slim 变体把它推给用户环境（用户自己的 Node 与 prebuilt addon 未签名/带 quarantine），这正是上游把"系统运行时装在受控路径之外"列为要避免的原因之一。两版本并存是对成本/体积的折中，而不是对安全的折中。

### 7.3 CI

两条 workflow，职责分离：

**A. `runtime-release`（跟随上游发版）**

```
schedule / 手动 → 探测 npm dist-tags
  → 取 @deepseek-ai/dsh@<new> → 产出闭包 tarball + sha256 + runtime.json
  → 发布到本仓库 release（tag: runtime-v<dsh-version>）
  → 更新 cask 的预装版本（可选，取决于是否想让 brew 用户自动跟随）
```

**B. `runtime-master`（跟随 harness main）**

```
harness main 有新提交 → checkout 上游源码
  → pnpm install --frozen-lockfile && pnpm run build
  → pnpm deploy --legacy --prod（复用 scripts/build-exe-for-python-sdk.ts 的配方）
  → 打 tarball + sha256 + runtime.json（channel=master, dshVersion=0.1.6-dev.<sha>.<date>）
  → 发布为 prerelease（tag: runtime-master-<date>-<sha>）
  → 更新 dsh-app@master cask
```

**C. `app-release`（壳自身的发布）**

```
macos-14 / macos-13 两个 runner（arm64 / x64）
  → nix build .#dsh-app-bundled（预装默认运行时）
  → codesign（清单见 7.2）→ notarytool submit --wait → stapler
  → hdiutil create → dmg
  → 生成/更新两个 cask → 推送 tap
```

`app-release` 需要 §7.2 那张表的全部凭据（`DSH_APP_SIGNING_IDENTITY` / `DSH_APP_TEAM_ID` / `APPLE_API_KEY{,_ID,_ISSUER}` / `DSH_APP_APP_ID` / `MACOS_CERT_P12` + `CERT_PW`），命名与上游一致。

**注意**：壳与运行时的发布节奏**解耦**了（这正是 D6 契约化的收益）——上游频繁发 dsh 时不需要重建壳，壳的签名/公证成本不随 dsh 发版次数增长。代价是"壳 + 最新运行时"的组合不再每次都被端到端验证过，因此**运行时安装后必须做一次冒烟启动**（起子进程 → 等到 URL 行 → 立即停止），冒烟不过就不激活该运行时。这一步是点锁步被降级后必须补上的验证。

---

## 8. 安全模型

| 面 | 处理 |
|---|---|
| 子进程 ↔ WebView | loopback + launch token → HttpOnly `SameSite=Strict` cookie；Host/Origin 栅栏由上游实现 |
| 未鉴权的 `/plugins/*` 与 SSE | 接受（只泄漏模块图）；随机端口降低可达性；记录为已知风险 |
| 自定义 scheme / 外部导航 | 不放行；窗口导航白名单只含 `127.0.0.1:<port>`；外链交系统浏览器 |
| 环境注入 | 白名单 + 剔除 `NODE_OPTIONS` / 包管理器变量 |
| 插件安装 | 复用内置 pnpm + 私有 store（`$DSH_HOME/dsh-app/pnpm`），前置到 PATH 供 `dsh plugin` 使用；绝不使用系统 pnpm |
| 凭据 | GUI 的 `home/.credentials.yaml` 保持 0600（上游启动时校验，组/他人可读会中止）；**默认与 CLI 的 `~/.dsh/.credentials.yaml` 分离**（D7），避免 GUI 与 CLI 跑不同后端版本时共用一份凭据与配置 |
| 多 preset 的文件竞争 | 按 D3.4 的四条不变量实现：启动串行化（I2）、插件操作全局互斥（I3）、每 preset 独立重启预算（I4）；实现时把不变量的断言写进代码而非注释 |
| 进程边界 | 单实例锁（只锁壳）+ 进程组 + SIGTERM/SIGKILL 升级；退出顺序：停全部 preset → 释放锁 |

---

## 9. 实施里程碑（每步可验收）

| 里程碑 | 交付 | 验收标准 |
|---|---|---|
| **M0 骨架** | `src-tauri/` + 占位 UI + 单实例 + 日志 | `nix develop` 内 `cargo tauri dev` 起窗口 |
| **M1 运行时闭包** | `scripts/prepare-dsh-closure.mjs` + doctor | 裸 Node 下 `node <closure>/lib/bin.js --profile dsh-app --no-open --port 0` 打印带 token 的 URL，浏览器能打开 GUI |
| **M2 壳托管** | preset 解析 + LaunchPlan + Web carrier + 就绪/退出 | GUI 在 WebView 内可用；关窗后无残留进程（`pgrep -f dsh-app` 为空） |
| **M3 preset 体系** | preset 文件 + UI 选择 + profile 物化 + patch 层 | 新建 preset 后能用 `dsh --profile <name>` 从 CLI 打开同一 profile（证明落到上游语义） |
| **M4 bundled 变体** | Node 下载/校验 + 三级探测 + 回退提示 | 摘掉系统 Node（清 PATH）后仍能启动；把配置指向坏路径时自动回退且 UI 有提示 |
| **M5 更新与回滚** | `channel.rs` + `runtime_store.rs` + 强制冒烟 + `current`/`prev` 原子替换（D8.2） | 能从当前版本更新到该通道最新版并回滚；塞一个坏版本后冒烟拦截、现状不变 |
| **M6 master 通道** | `runtime-master` workflow + `dsh-app@master` cask | master cask 装完可用；能更新到更新的 master 产物；能回滚 |
| **M7 Nix 产物** | `.#dsh-app-bundled` / `.#dsh-app-slim`（`cargo-tauri.hook`，`beforeBuildCommand` 留空） | `nix build` 产物双击可运行（未签名，Gatekeeper 需右键打开） |
| **M8 签名公证** | 逐文件签名脚本 + Node entitlements + notarytool + dmg（**先做端到端试签**，路线 1/2 二选一）。**进入 M8 才需要 Apple 开发者账号与 §7.2 凭据** | `spctl -a -vvv` 通过；staple 后冷启动冒烟通过（**含一次真实 PTY 操作**）；签名前后 shasum 一致 |
| **M9 brew** | 两个 cask 生成 + tap 更新 | 干净 macOS 上 `brew install --cask dsh-app` 与 `…/dsh-app@master` 均装完即用 |
| **M10（v1.1）** | 多 preset 并发托管（D3.2）+ `kind: task`/`server`（D3.3）+ Tauri updater | 能同时跑两个 preset（各自会话互不干扰）；`task` 能显示退出码与输出；旧壳能自更新 |

---

## 10. 风险与待决问题

| # | 风险 / 问题 | 影响 | 处置 |
|---|---|---|---|
| R1 | `spawn` 出的 dsh 子进程在壳被 `SIGKILL` 时可能残留。**Tauri 明确不会回收 Rust 侧 spawn 的子进程**（A.3），且 `kill` 不做进程树清理 | 端口占用、profile 写租约冲突、孤儿进程 | 用 `std::process::Command` + `process_group(0)` + 杀进程组；子进程侧 stdin-EOF / `disconnect` 兜底；M2 验收显式测（`pgrep -f dsh-app` 为空） |
| R2 | Node 版本/ABI 漂移导致 prebuilt addon 加载失败（尤其 slim 变体） | 启动失败 | doctor 前置探测（D2），失败回退 bundled |
| R3 | 上游把 `dsh web` 的 stdout 行格式改掉 | 就绪解析失效 | 解析用宽松正则 + 单测；**安装后强制冒烟（D8.2 U1）是真正的门禁**；必要时向上游确认该行的稳定性承诺 |
| R4 | 未鉴权静态面（`/plugins/*`、SSE） | 本机信息泄漏 | 记录；随机端口降低可达性；**不再有"改用 Pipe carrier"这条退路**（A.1），若确需收敛应向上游提 |
| R5 | **[改]** 原假设"手搓 `.app` vs 官方 bundle 是二选一"被 A.4 改写：官方 bundle **根本不会签 Resources**，所以两条路线都必须自己签 Resources | 签名遗漏 → 公证失败 | M8 先做端到端试签；默认路线 1（自己组装 + 自己签）绕开 tauri#11992；路线 2 作为备选 |
| R6 | 公证对"未签名/异常嵌套 Mach-O"极严格；Tauri 按扩展名跳过 `.node`，`--deep` 到不了 Resources | 发布阻塞 | **把端到端试签提前到 M4 之后立刻做**，不要等到 M8；签名前后 shasum 比对 |
| R7 | `dsh plugin` 依赖 PATH pnpm 的隐式契约 | 插件管理在 GUI 内不可用 | 前置内置 pnpm；M3 验收包含一次真实插件安装 |
| R8 | 用户 preset 引入需要 HMR 的行且 Node 无 internals | 启动抛错 | doctor 结果 + preset 校验期提示；不静默降级到 `--expose-internals` |
| **R9** | 共享回退争用（`profiles/node_modules` 镜像本次安装的依赖闭包） | 两个不同 dsh 安装并发时随机失败 | D8 的"一次安装一个版本"从结构上消除（D3.4 I1）；实现时仍须断言所有 preset 解析到同一 runtime-id |
| **R10** | **契约漂移**：新运行时改变了壳依赖的行为（尤其就绪行格式），而壳不再与后端点锁步 | 更新后装上了但起不来 | **安装后强制冒烟**（不过则丢弃 staging、现状不变）+ `prev/` 回滚点；解析宽松 + 单测（D6） |
| **R11** | master 通道把上游未发布缺陷带给用户 | 体验/数据风险 | UI 显式标注通道 + commit sha；更新前提示、失败自动保留 `prev/`；master 由独立 cask 承担（默认 cask 永远是 release） |
| **R12** | 运行时下载面（用户机上的网络 + 完整性） | 供应链风险 | 只从固定的 npm registry / 本仓库 release 拉取；强制 sha256 校验后才激活；拒绝任意 URL |
| **R13** | wry#1778：macOS 26.6 beta 报"跨自定义 scheme 的子资源加载失效"（尚未确认是否进入正式版） | 若命中会影响所有用自定义 scheme 的 Tauri 应用 | **本设计不使用自定义 scheme**（A.1/A.2），天然规避；仅作为"不要回头去用自定义协议"的又一条理由记录 |
| Q1 | `kind: server` 在 v1 是否值得做？它没有可用的附着 UI，只是"配置与进程的容器" | 决定 v1 工作量 | 待评审：可以先只做 `web` + `task`，`server` 留 schema 不实现 |
| Q2 | slim 变体是否也发 dmg（体积小）还是只发 tarball？ | brew formula 的实现方式 | 待定 |
| Q3 | 是否要向上游提一个 `dsh-app` 专用的 reserved profile 名（避免与 CLI 撞名） | 生态一致性 | 待定，可与上游沟通 |
| **Q4** | **多 preset 同时跑**：v1 是否就允许（UI 只有单活动窗口），还是先限制为"一次一个"？ | 决定 supervisor 与并发不变量的实现时机 | 待评审：架构按"可并发"设计（D3.2/D3.4），但 v1 可以先只暴露单活动 |
| **Q5** | GUI 的 home 是否与 CLI 共享 | 影响与 CLI 的协作体验 | **已定：打包态直接用 `~/.dsh`**（D7），不另设开关——GUI 与 CLI 是同一产品的两个前端，共享会话与凭据才是默认语义；要隔离用 `DSH_APP_HOME` |
| **Q6** | 更新检查是自动（启动时静默检查）还是纯手动？ | 影响网络行为与隐私观感 | 待评审：建议启动后延迟检查 + 只提示不自动装 |

---

## 11. 实测记录（M1 已完成）

`pnpm runtime:prepare` + `pnpm runtime:probe` + `pnpm runtime:run` 三条命令构成 M1 的可执行验收。
下列结论来自 2026-09-16 在 arm64 macOS 上的实际运行，**其中三条改写了设计假设**。

### 11.1 闭包构建（`scripts/prepare-runtime.mjs`）

| 事实 | 说明 |
|---|---|
| 闭包大小 **24473 文件 / 209.6 MiB** | release 0.1.5-rc.1；这是 bundled 变体的运行时基线 |
| `pnpm deploy` **拒绝 workspace 根** | 报 `ERR_PNPM_NOTHING_TO_DEPLOY`；闭包项目必须是 workspace 成员（上游 Python runtime 正是这个形状）。已改为 `runtime/` 子包 + `--filter` |
| **必须 `--config.auto-install-peers=true`** | [改] 照抄上游 SEA 配方的 `false` 会让闭包缺 **24 个 peer 依赖**，首次启动即 `ERR_MODULE_NOT_FOUND: @deepseek-ai/cordis-plugin-group`。发布包把大量 harness 包声明为 peer |
| 上游 profile 的 `pnpm-workspace.yaml` 写的是 `autoInstallPeers: false` | 这不是矛盾：profile 侧故意关掉，靠 `profiles/node_modules` 回退共享同一份 cordis 实例；**closure 侧必须开**。两边职责不同，已在 `scripts/lib/profile-template.mjs` 里注明 |
| `--ignore-scripts` 下 `spawn-helper` **仍是可执行的** | 上游那个 postinstall 的效果在 deploy 产物里保留了；doctor 会显式检查这一点 |
| 上游 postinstall 被忽略的包 | `@deepseek-ai/dsh-subprocess-local`、`@google/genai`、`koffi`、`node-pty`、`protobufjs` |

### 11.2 实测通过的三种启动形态

| # | Node | profile | internals | 结果 |
|---|---|---|---|---|
| A | 官方 24.17.0 | `web`（`patchReload: live`） | addon | ✅ 就绪行 / 303 / 匿名 401 / SIGTERM→code 0 |
| B | Nix `nodejs-slim` 22.22.3 | `web` | `--expose-internals` | ✅ 同上 |
| C | Nix `nodejs-slim` 22.22.3 | **自建 `dsh-app`（`startup`）** | **不需要** | ✅ 同上 |

形态 C 是**我们的默认配置**，因此默认路径不依赖 internals 能力。

### 11.3 [改] internals 能力绑定在二进制上，不只是版本

`node-addon-require-builtin` 靠匹配机器码工作。实测：

- 官方 nodejs.org 构建 → `requireBuiltin('internal/modules/esm/loader')` 正常返回；
- Nix `nodejs-slim`（同一大版本 22）→ addon 可 require 但**拒绝所有 id**：
  `Unsupported/no-getter (arm64 getter is not optional-bti-ldr-x0-this-imm-ret)`。

两条设计后果：

1. **bundled 变体必须用官方 nodejs.org 归档**（与上游 `prepare-runtime.ts` 下载 nodejs.org 的做法一致）。发行版重打包的 Node 可能不被 addon 支持。
2. **doctor 必须真的调用 `requireBuiltin` 并检查返回值**——只检查"能否 require 该模块"会给假阳性，把失败推迟到 profile 挂载期，并报出一句误导性的 "`--expose-internals` is required"。

`--expose-internals` 是实测可用的等价兜底，仅在 profile 需要 internals（`patchReload: live`）且 addon 不支持时启用。

### 11.4 验收断言（已固化为脚本行为）

`scripts/run-runtime.mjs` 现在会失败于任何偏差，而不只是打印：就绪行正则、`index?token=` 必须 302/303、匿名 index 必须 401、SIGTERM 后必须 exit code 0。

### 11.5 preset → 启动的映射：patch 层用 `--patch`，不碰用户的 patch 文件

M3 实现时确认了"薄封装"的具体载体。上游的 patch 层栈是（`apps/cli/src/profile-boot.ts:207-216`）：

```
bundle patches → profile 的 cordis.patch.yml → $DSH_HOME/cordis.patch.yml → --patch 覆盖 → telemetry
```

`--patch <path>` 可重复、按 argv 顺序，正好落在**用户层之上**，所以 preset 的 patch 不需要（也不应该）写进 `profiles/<p>/cordis.patch.yml`——那会把用户自己的文件变成我们的托管文件。启动命令因此是：

```
[--expose-internals] <entry> --profile <p> --patch <f1> --patch <f2> --no-open --port 0
```

launcher flag（`--profile` / `--patch`）必须在前，app flag（`--no-open` / `--port`）在后，因为上游语法是"第一个不认识的 token 起归 app"。

**实测确认的语义**（容易踩）：patch 的 `config` 是**整对象替换**，不是深合并。只写 `port` 会丢掉 `host`，而 `webserver` 的 schema 里 `host` 与 `port` 都是 required，于是启动直接报 `invalid config`。preset 模板注释里已写明。

另外确认了 patch **确实压过 app flag**：用 patch 把 `port` 固定成 45671 后，即使 argv 里带着 `--port 0`，实际绑定的仍是 45671。

### 11.6 profile 物化模板（已抽成共享模块）

`scripts/lib/profile-template.mjs` 镜像上游 `PROFILE_PATCH_TEMPLATE` / `PROFILE_PNPM_WORKSPACE` / `initProfile` 的产物形状。两个易错点：空 patch 文件必须是**顶层 YAML 数组**（`[]`，零字节文件会报 "must be a top-level YAML array"）；profile 的 `pnpm-workspace.yaml` 必须带 `nodeLinker: hoisted` 与 `autoInstallPeers: false`。

## 12. 壳页面运行的两条硬约束（血泪实测）

实现 M3 时壳页面**完全没有执行**，表现为 Runtime 三格永远停在 HTML 的初始值 `—`。
根因是两个各自都能致命、且都**静默**的问题。记在这里，因为它们不是"踩过一次就不会再踩"的类型。

### 12.1 capabilities 是必需的，不是可选的

Tauri v2 的前端命令受 **capabilities** 权限系统管辖。`src-tauri/capabilities/*.json` 不存在时：
自定义命令（我们在 `invoke_handler` 里注册的）仍可调用，但**内建命令全部被拒**：

```
event.listen not allowed. Permissions associated with this command:
core:event:allow-listen, core:event:default
```

我们的页面第一件事就是 `listen('dsh://status')`，于是模块在顶层 await 处抛错，整个页面死掉，
而 `invoke('shell_state')` 排在它后面，**从未执行**——所以现象是"页面看起来加载了，但什么都没填充"。

修法：`src-tauri/capabilities/shell.json`，`windows: ["shell"]`（harness 窗口**刻意不给任何 capability**
——它跑在运行时的 loopback origin 上，是外部内容），权限只需 `core:default` + `core:event:allow-listen`
+ `core:event:allow-unlisten`。

注意 `build.rs` 的 rerun 规则里那条 `capabilities` 就是 `tauri-build` 为它发的——看到这条规则存在，
就该想到"这个目录是有语义的"。我把它当成噪音忽略了。

### 12.2 壳页面没有控制台，必须自己造一个

**ES module 求值期抛异常不会向宿主报告任何东西。** 没有 stderr、没有 Tauri 日志，什么都没有。
壳又是 GUI，终端看不到。两者相加 = 一个"看起来正常但什么都不做"的页面，这就是本次耗时最久的原因。

沉淀下来的两个机制（都保留在代码里）：

1. **页面自捕获**：`ui/shell.js` 只是一个 try/catch 外壳，用**动态 import** 加载真正的页面
   `ui/shell-page.js`。任何求值期异常都会被捕获、写进页面的 `#fatal` 区域、并挂到
   `window.__DSH_PAGE_ERROR__`。动态 import 是关键——静态 import 的失败发生在入口模块求值之前，
   包不住。
2. **宿主注入探针**：`DSH_APP_DEBUG_STATE=1` 时，壳体在页面加载后用 `webview.eval()` 注入一段探针，
   把 `__TAURI_INTERNALS__`/`__TAURI__` 的类型、已加载脚本、以及 `__DSH_PAGE_ERROR__` 读回来打到 stdout。
   `eval` 是宿主注入，**不受页面 CSP 约束**——这正是它比"在 HTML 里塞 inline handler"可靠的原因
   （那个方案被 CSP 挡了，还掩盖了真正的原因）。

### 12.3 顺带确认的两件事

- **`window.__TAURI__` 在裸 `cargo build` 下也存在**（探针实测 `global = object`）。它由运行时初始化脚本
  提供，不依赖 `tauri build`。但壳页面仍然改用 `__TAURI_INTERNALS__`（见 `ui/tauri.js`），因为后者是
  运行时无条件注入的，语义更明确。
- **`tauri-build` 不追踪 `frontendDist`**：`build.rs` 现在显式 emit `cargo:rerun-if-changed=../ui`，
  否则只改 `ui/` 不会重新嵌入。同时 `build.rs` 把页面内容哈希成 `DSH_APP_UI_VERSION` 并显示在
  Runtime 区——这样"当前跑的是哪一版页面"永远可判定。


## 13. 多 preset 并发：实测通过（M3 验收）

D3.2/D3.4 那套不变量第一次真实检验，结论是**成立**。隔离 home 下同时跑两个 preset：

```
dsh web: http://127.0.0.1:61068    ← dsh-app（--port 0，OS 分配）
dsh web: http://127.0.0.1:45677    ← second（patch 固定的端口）
```

| 检查项 | 结果 |
|---|---|
| 两个子进程同时存活 | ✅ 2 个 |
| 两个 profile 各自物化 | ✅ `dsh-app` / `dsh-app-second`，互不覆盖 |
| **I1 共享回退** | ✅ `profiles/node_modules` 的 symlink 全部指向**同一个** `release-0.1.5-rc.1`；payload 里两个 preset 的 `runtimeId` 一致 |
| patch 层生效 | ✅ `second` 用自己的 `patch.d/fixed-port.yml` 把端口钉在 45677，而 argv 里仍带 `--port 0` |
| 用户 patch 文件未被污染 | ✅ 两个 profile 的 `cordis.patch.yml` 都是上游模板原文（preset 的 patch 走 `--patch`，不写这里） |
| 退出清理 | ✅ 关闭后残留进程 0 |

### 13.1 两处实现细节（实现时定下来的）

- **`DSH_APP_HOME` 覆盖**：`config.rs` 的 `resolve_home()` 优先读它，其余按运行形态分：打包态（可执行文件位于 `<bundle>.app/Contents/MacOS/`）用 `~/.dsh`，开发 checkout 用仓库内 `.dsh-build/home`。
  这是隔离验证（`DSH_APP_HOME=$(mktemp -d)`）的开关；打包态本身就是用户的 harness home，GUI 与 CLI 直接共享产品数据（D7/Q5）。
- **preset 目录是整棵树 seed 的，不只是 `.toml`**：`second.toml` 通过 `patches = ["patch.d/fixed-port.yml"]`
  引用**相对自己**的补丁，所以 `patch.d/` 必须一起落到 home。`build.rs` 把 `src-tauri/presets/`
  拷进 `OUT_DIR/seed-presets/`（编译期常量路径——从可执行文件上溯推路径在 `target/<profile>/` 与
  `Contents/MacOS/` 下深度不同，猜错就会静默不 seed），首次运行 `seed_missing()` 只补缺失文件，
  用户改过就不再覆盖。→ **§22 已删除这套 seed**：壳不再往 home 里放任何 preset，
  示例留在 `src-tauri/presets/`（附 README），要用自己拷。

### 13.2 待处理的两个小问题

- **自动启动的 preset 是"排序后第一个"**，不是用户上次选择或显式标记的那个。当前 `dsh-app` 因为
  字母序在前而自动启动；如果以后 seed 了更靠前的 id，行为会默默改变。应改成显式声明（preset 里加
  `default = true`，或记住上次选择）。→ **已由 §21 处理：自动启动整个去掉了。**
- **列表不热监听**：新增 preset 文件后必须点 "Reload presets"。shell 已经有 `reload_presets` 命令，
  接一个 `notify` 监听是后续小改动。


## 14. M4 完成：bundled Node 与 internals 决策链

### 14.1 Node 归档：校验链不落在本仓库里

`scripts/prepare-node.mjs` 从 nodejs.org 取归档，用**该 release 自己的 `SHASUMS256.txt`** 校验，
不把哈希抄进仓库（抄进来就会悄悄腐烂）。默认 pin `24.17.0`，版本号存在 `package.json` 的
`dshRuntime.nodeVersion`，与 dsh 版本同一个 manifest 维护。

解压时排掉 `include/`、`share/` 与文档——它们对运行时无用，却会让签名清单和体积无谓变大。
实测：sha256 校验通过，归档自报版本与 pin 一致才发布。

### 14.2 三级探测：官方归档必须压过 PATH

`config.rs` 的解析顺序（D2）现在是：

```
1. DSH_APP_NODE                      显式覆盖
2. .dsh-build/node/<pin>/bin/node    内置官方归档（pin 优先，再取其余已 staged 的）
3. PATH 里的 node                     兜底
```

第 2 级必须排在第 3 级之前，理由不是"自包含更好看"，而是**功能性的**：只有官方构建能提供
`require-builtin`（见 §11.3）。实测确认解析结果已从 Nix 的 `nodejs-slim` 切到
`.dsh-build/node/24.17.0/bin/node`。

### 14.3 internals 决策：只在需要时付代价

`node_probe.rs` 对候选 Node **真的调用** `requireBuiltin('internal/modules/esm/loader')`
（按 (可执行文件, 闭包路径) + mtime 缓存），然后由 preset 的 `patchReload` 决定要不要 flag：

| Node | addon | preset `patchReload` | 结果 |
|---|---|---|---|
| 官方 24.17.0 | ✅ | `live` | 启动 argv **无** flag，`internalsViaFlag=False` |
| Nix 22.23.2（`DSH_APP_NODE` 指定） | ❌ | `live` | 自动加 `--expose-internals`，`internalsViaFlag=True` |
| 任一 | 任一 | `startup`（默认） | 完全不请求 internals，不付额外权限面 |

flag 走 **argv**（`node <flag> <entry> …`）而不是 `NODE_OPTIONS`——后者是被刻意从继承环境中剔除的
变量，把决策塞回去会让"child env 干净"这条约束出现例外。UI 通过 `internalsViaFlag` 显示当前是否
在用降级路径。

### 14.4 顺带修掉一个 doctor 的假阴性

`probe-runtime.mjs --require-internals` 原本在 Nix Node 上**误报 OK**：只要 `--expose-internals`
可用就返回 ok，把"addon 不支持"吞掉了。现在把 `how` 的差异保留到判定里：官方 Node → OK；
Nix Node 加断言 → **FAIL 并 exit 1**；不加断言 → OK 但输出 note。这条断言正是 `live` preset
能不能跑的判据。


## 15. M5 完成：运行时存储、强制冒烟、回滚

### 15.1 种子路径改为运行时解析（推翻 D8.2 U3 的一处实现细节）

D8.2 原本设想"首次启动做一次 seed → home 的复制"，实现时发现**种子路径不能靠构建期 staging**：
闭包 210 MB / 24k 文件，每次构建复制它会主导构建时间；而 `build.rs` 又不会在
`pnpm runtime:prepare` 重建闭包后自动重跑（我已经为 `ui/` 和 `presets/` 加了
`rerun-if-changed`，再加一条 210 MB 的复制只会更糟）。

改为 `config::seed_runtime_dir()` **在运行时解析**，按布局依次尝试：

```
<bundle>/Contents/Resources/runtimes/<id>/    打包后（bundle.resources 声明）
<repo>/.dsh-build/runtimes/<id>/              开发构建
```

`build.rs` 只创建空的 `src-tauri/gen/seed-runtime/` 让资源声明合法。打包时由 M7/M9 的脚本
把闭包拷进去。**代价**：开发构建同样会执行 seed 安装（从 `.dsh-build/runtimes`），一次 210 MB
的复制；换来的是构建时间不受影响。

### 15.2 强制冒烟落在哪

冒烟是 `activate()` 的**前置条件而不是它的一部分**：`activate()` 只做原子切换，调用方负责先证明
候选能起。理由是这样 store 保持"原语"语义，而"新运行时投入服务必须先验证"这条规则由 `lib.rs`
的 seed 路径执行——将来 M6 的下载路径复用同一个门禁。

冒烟用**独立的 scratch profile**（`<home>/dsh-app/smoke/profiles/dsh-app-smoke`），绝不碰用户的
`profiles/<preset>`：一次验证不能扰动正在使用的 profile。它固定用 `patchReload: startup`，
因为 internals 是 profile 的属性、不是运行时的属性。

实测（三条路径）：

| 场景 | 结果 |
|---|---|
| 正常 seed | 过冒烟 → 激活 → `current -> release-0.1.5-rc.1` |
| 坏运行时（entry 指向缺失文件） | 冒烟/启动失败 → preset 转 `failed` 并把子进程 stderr 尾部交给 UI |
| 坏版本处于 active 时执行 `--rollback` | 切回上一个版本 → 壳正常 `ready` |

### 15.3 `--rollback` 是恢复路径，不是测试后门

壳起不来时用户需要一个终端可达的出口，所以回滚做成了命令行开关（`dsh-app --rollback`），
GUI 里同时有一个按钮（两者都受"先停掉所有 preset"约束）。这不是为了方便测试才加的——
加它是因为**只有 GUI 一条恢复路径本身就是个设计缺陷**。

### 15.4 一处实现修正

`installed` 列表原先把 `current` / `previous` 两个软链也算成了版本。改用 link-aware 的
`file_type()` 判定。这类错误不会让功能失效，但会让人误判 store 里有什么。

### 15.5 M5 的边界（尚未做的部分）

- **`channel.rs` 的 `check()` / `update()` 未实现**：目前没有任何网络路径，运行时只能来自
  种子。真正"追新"要等 M6（master 通道的 CI 产物）与 release 通道的下载实现。
- 因此**"更新"这条路径本身还没有端到端验证过**，只有它的两个端点（安装、激活/回滚）验证过。
- UI 只有"回滚"按钮，没有"检查更新"。


## 16. M7 完成：Nix 产出 `.app`

```
nix build .#dsh-app-slim       # 壳 + 种子运行时，Node 走 PATH
nix build .#dsh-app-bundled    # 上者 + 官方 Node（app 415MB，其中 Node 132MB）
nix build .#dsh-runtime        # 仅闭包，供 CI 自行打包
nix build .#node               # 官方 Node 归档（已固定哈希）
```

产物是未签名的 `.app`（右键打开即可），预装种子运行时与样例 preset。
实测两个变体的差异精确符合预期：

| 变体 | 解析到的 Node |
|---|---|
| bundled | `<app>/Contents/Resources/node/bin/node` |
| slim | PATH 上的 node |

### 16.1 为什么不走 `tauri build`（§5.2 的取舍落地）

壳页面是三个静态文件，真正的 GUI 由运行时在运行时经 HTTP 提供。所以
`frontendDist` 不需要工具链、`beforeBuildCommand` 为空，直接 `cargo build`
再用 `packaging/assemble-app.sh` 组装 bundle 即可。

那个脚本**本地打包与 Nix 共用**：bundle 布局是契约（壳从
`Contents/Resources/runtimes` 找种子、从 `Resources/node/bin/node` 找 Node），
两份实现必然漂移。

### 16.2 沿用而不是复述

闭包构建**复用 `scripts/prepare-runtime.mjs`**（新增 `DSH_BUILD_ROOT` 重定向输出），
而不是在 Nix 里重写 pnpm deploy 的 flag。理由与 §11.1 那条 `auto-install-peers`
同源：那些 flag 正是最容易在第二份实现里悄悄烂掉的部分。

### 16.3 三个让构建静默失败的坑

1. **`packageManager: pnpm@11.7.0` 会让 pnpm 自我切换版本。** flake 提供的是
   11.25.0，pnpm 读到该字段后去下载 11.7.0；无网沙箱里它**什么都不做、退出码 0**，
   于是 `pnpm deploy` 静默不产出。修法：flake 自己钉 `pkgs.pnpm`，删掉该字段
   （工具链版本由 flake 声明，不该由 package.json 的字段间接决定）。
2. **`runHook preBuild` 在没有对应 hook 时返回 1**，而 builder 跑在 `set -e` 下，
   整个 buildPhase 静默中止——表现为若干步之后一个看似无关的 `chmod` 失败。
   本仓库的 derivation 不再调用 `runHook`。
3. **`cp <文件> <目录>/` 保留源文件名**，而 Nix store 路径带哈希前缀：产出
   `1sc98ak…-assemble-app.sh`，更糟的是种子会落进以哈希命名的目录、壳按
   manifest 里的版本号找不到。所有目标路径改为显式命名。

### 16.4 顺带修掉的缺口

`resolve_node` 原先**不查 bundle 内的 Node**，在用户机器上会跳过刚打进去的那份——
那会让 bundled 变体完全失去意义。现在顺序是：`DSH_APP_NODE` → `.dsh-build/node`
→ **bundle 内的 `Resources/node`** → PATH。

启动时新增一行日志记录解析到的 runtime 与 node：打包后 Home 不再是仓库，
"它到底用了哪个 Node"是排障第一问。

### 16.5 M7 的已知未完成项

- **闭包 derivation 不是 FOD**：它从 npm 解析，且**没有提交 lockfile**，
  所以两次构建可能得到不同的 `@types/node` 版本（实测差 24 个 `.d.ts`，
  无运行时影响）。用 `fetchPnpmDeps` + 离线安装固定下来是后续工作。
- **`CFBundleShortVersionString` 仍是 0.0.0**：版本策略要等 M9 把 cask 的
  版本语义定下来（D10 已定：cask version = 壳版本）。
- 未做 DMG（`hdiutil` 在沙箱内未验证）、未签名、未公证。


## 17. M6 完成：通道检查与运行时更新

M5 只验证了"安装"与"回滚"两个端点，中间的**更新**这一整段是空的。M6 补上了它。

### 17.1 发布清单格式（壳与 CI 之间的接口）

`channel.rs` 读取一份很小的 JSON：

```json
{
  "channel": "release",
  "releases": [
    { "version": "0.1.6", "platform": "darwin-arm64",
      "tarball": "https://…/dsh-runtime-0.1.6-darwin-arm64.tar.gz",
      "sha256": "…", "shellContract": 1 }
  ]
}
```

**按平台分别发布，而不是一份通用归档**：闭包在构建它的机器上解析原生包
（`node-pty`、`koffi`、`require-builtin` addon），一份通用归档等于把某个平台的二进制
发给所有平台。反过来"在用户机器上解析"又意味着要带上 pnpm 与 store——那正是展开式闭包
要避免的东西。

`shellContract` 让运行时声明它需要的壳能力（D6 的契约）；壳拒绝高于自身契约的条目。

### 17.2 `scripts/package-runtime.mjs` 是三处约定的唯一生产者

归档布局、平台标签、摘要都由它产出，因为三者必须与 `channel.rs` 一致。**平台标签统一
为 Node 的拼写**（`darwin-arm64`，不是 Rust 的 `macos-aarch64`）——它同时出现在
`runtime.json`、官方 Node 归档名和打包脚本里，出现第二种拼写就会静默失配。这个不一致
在实现时真的出现了，并已修正。

同时给下载加了一道平台校验：发布归档是平台特定的，拿错平台应当在激活前被拒，而不是
在首次启动时表现为 ABI 失败。

### 17.3 实测（端到端）

用本地 `file://` 渠道与桩运行时（会打印就绪行，因此就绪解析与冒烟门禁都被真正走过）：

| 场景 | 结果 |
|---|---|
| `--check-update` | `0.1.5-rc.1 → 0.1.6`，`update_available: true` |
| `--update-runtime` | activated `0.1.6-darwin-arm64`、smoke_url 已脱敏、`replaced` 记下旧版 |
| 打包脚本产出的归档（`0.1.9`）再走一遍 | 同样成功 —— 打包与更新两侧用的是同一份约定 |
| 摘要不匹配 | **拒绝**，报出 expected/actual，`active` 不变，无残留文件 |
| 平台不匹配 | **拒绝**（"the published runtime is for … but this machine is …"），现状不变 |
| `--rollback` | 退回 `release-0.1.5-rc.1` |

更新后 `previous` 指向被替换的版本，所以"更新坏了就退回去"这条链是闭合的。

### 17.4 `--check-update` / `--update-runtime` 是能力而非测试后门

与 `--rollback` 同源的理由：**更新恰恰是 GUI 不可用时用户要做的事**，只留 GUI 一条路径
是设计缺陷。它们也是这条路径能在没有窗口的情况下被验证的原因。

### 17.5 已知未验证的部分

- **CI 工作流（`.github/workflows/runtime.yml`）从未运行过**：它产出的归档格式与
  `package-runtime.mjs` 一致（本地已测），但 workflow 本身（Nix 安装、双平台矩阵、
  artifact 合并、gh release 上传）没有执行过。合并步骤里"多平台合成一份 channel 文档"
  尤其未经真实运行。
- **`dsh-app@master` 的 cask 与 master 通道的 harness 源码构建**未做：当前 master 只是
  工作流里的一个参数分支，它会构建**本仓库的闭包**而非 harness `main`。真正跟随
  harness main 需要在 CI 里 checkout 上游并构建，这是 M6 明确遗留的部分。
- 渠道 URL 目前只能由 store.json 手工写入；"安装时按 cask 决定渠道"要等 M9。


## 18. 平台与发布路径收窄（用户决定）

### 18.1 只支持 Apple Silicon macOS

用户明确：**不做 x64，其他 macOS 也不支持**。落地为：

- `flake.nix` 从 `eachDefaultSystem` 收窄到 `eachSystem ["aarch64-darwin"]`，`platformTag`
  变成常量 `darwin-arm64`（不再有 `isArm` 分支）。在别的系统上求值会得到
  "flake does not provide"，这是诚实的答案——闭包为构建它的机器解析原生包，**不存在正确的交叉构建**。
- CI 矩阵收成单平台。

收窄本身消掉一类隐患：`channel::platform_tag()`、下载期平台校验、打包脚本的平台校验、
`runtime.json` 四处现在永远一致，并且 `pnpm` 解析 `node-pty`/`koffi`/`require-builtin`
的 optional 包时只有一个正确答案。

### 18.2 [改] GitHub 托管 runner 的现实（前一轮我写错了）

查 `actions/runner-images` 后确认：

| 镜像 | 架构 | 标签 |
|---|---|---|
| macOS 26 / 15（arm64） | arm64 | `macos-latest` / `macos-26` / `macos-15` |
| macOS 26 / 15（Intel） | x64 | `macos-26-large` / `macos-15-large` — **larger runner，付费** |
| macOS 14 | arm64 | 已 deprecated |

我原先写的 `macos-14`（已 deprecated）与 `macos-13`（已退役）都是错的；更要紧的是
**免费的标准 macOS runner 只有 arm64**，所以那份双平台矩阵根本产不出 `darwin-x64`。

### 18.3 CI 降级为"可选、手动"

用户决定：**不用 CI 打包**，本地来。`.github/workflows/runtime.yml` 保留但：
去掉 `schedule`（不再有无人值守的运行）、矩阵收成单平台、runner 可通过 input 覆盖为自托管、
文件头写明它是**可选手动路径而非发布流程**，并记录托管 runner 的三个约束（约 14 GB 磁盘、
Nix 多用户 daemon 是脆弱环节、没有免费 Intel）。

保留它的理由：那些约束是踩出来的，删掉文件就得重新踩一遍。

### 18.4 发布路径重构：不要把东西从 store 里复制出来

这一轮我在这上面反复失败（连续几轮都在给同一个权限问题打补丁），根因值得写下来：

**Nix store 的路径是只读的（`0555`），而 `fs.cp` / `cp -R` 会连目录权限位一起复制。**
于是"从 store 复制一个 bundle 出来"总会得到一个只读副本，后续任何写入（tar 清理、
`codesign`）都以 `EACCES` 失败。我先后试了 `mode: 0o755`（无效——`fs.cp` 的 `mode`
只作用于它写入的文件，不作用于目录）和递归 `chmod`（有效但仍是补丁）。

**真正的修法是不要复制**：

```
dist/                                    ← 只放可移植且只读的东西
  dsh-runtime-<ver>-<platform>.tar.gz      updater 的载荷
  channel-<channel>.json                   channel 文档
  release.json                             本次发布的摘要
```

`.app` 不进 `dist/`。store 路径**本来就能直接运行**（只读的东西我们只读地用），
而"做成可写副本"只对**签名**有意义——那是 `packaging/export-app.sh` 的职责，
在真正拿到 Developer ID 时才跑（§7.2）。放在发布脚本里会让每次本地打包都产出一个
看起来能发布、其实未签名的可变产物。

### 18.5 顺带修掉的一个静默覆盖

`package-runtime.mjs` 写的 channel 文档名叫 `<channel>.json`，而 `package-release.mjs`
的摘要在同一目录写 `release.json`——**channel 为 release 时两者同名，摘要静默覆盖了
updater 需要的文档**。已改名为 `channel-<channel>.json`。

这个 bug 的危险之处在于它是静默的：打包看起来成功，只有壳去 fetch 时才会发现文档是错的。

### 18.6 实测

| 步骤 | 结果 |
|---|---|
| `nix build .#dsh-runtime` | 收窄 flake 后仍能构建 |
| `node scripts/package-release.mjs` | `dist/` = 运行时归档（51.7 MB）+ `channel-release.json` + `release.json` |
| `packaging/export-app.sh bundled <dir>` | `dr-xr-xr-x` → `drwxr-xr-x`，可签名 |
| 导出的 app 启动 | 装种子、过冒烟，并用**自己 bundle 里的 Node**（`Contents/Resources/node/bin/node`） |


## 19. 签名完成（不含公证，按用户决定）

`src-tauri/packaging/sign-app.sh` 已用真实 Developer ID 跑通。**11 个 Mach-O + app 本身**
全部带上 `TeamIdentifier=XQYB6GQJJ5`，`codesign --verify --deep --strict` 通过，
签名后冷启动装种子、过冒烟。

### 19.1 只有签名才会暴露的两个问题

这两个都是**在签名之前完全正常**的，所以一直没被发现——这也说明"签名后冒烟"必须留在流程里。

**① `libiconv` 链接到 Nix store 的绝对路径**

```
dyld: Library not loaded: /nix/store/…-libiconv-115.100.1/lib/libiconv.2.dylib
      … (non-platform) have different Team IDs
```

壳二进制里有**一个**非系统依赖（`libiconv`），指向 store 的绝对路径。两重致命：

- 那是**这台机器**的路径，换台机器直接找不到；
- 即使找到，签名后 hardened runtime 的 library validation 会拒绝**不同 Team ID** 的动态库，而 Nix store 里的库都是上游签的。

修法在 `dshApp` 的 `installPhase`：`install_name_tool` 把它改写为 `/usr/lib/libiconv.2.dylib`。
该文件在磁盘上不存在，但 macOS 从 **dyld shared cache** 提供它，在那里它是 platform binary，
永远可加载。改写后加了断言：**store 依赖数必须为 0**，否则构建失败。

**② 打进 bundle 的 Node 丢失全部 entitlements**

官方 Node 有 6 项，`install` 进 bundle 后**变成 0 项**——复制二进制会让签名失效、entitlements 随之消失。
而 `codesign --preserve-metadata=entitlements` 是**从这份已经没有 entitlements 的副本里抽取**，
于是静默产出空文件。后果：V8 起不来，报 `Fatal process out of memory: Failed to reserve
virtual memory for CodeRange`（JIT 需要 `allow-jit`）。

修法：entitlements **从 store 里那份原始官方 Node 显式提取**
（`packaging/entitlements/extract-node-entitlements.sh`），并去掉 `get-task-allow`。
脚本随后**断言**三项必须在（`allow-jit`、`allow-unsigned-executable-memory`、
`disable-library-validation`）、`get-task-allow` 必须不在。

这是同一类错误的**第三次**出现（seed 目录名、`install` 不保留权限、这次是 entitlements）：
**Nix 的 store 里的东西是对的，经过一次复制就不再是原来那个东西。**

### 19.2 签名清单与顺序

12 个 Mach-O（壳、Node、10 个原生扩展），逐个签：

```
Contents/MacOS/dsh-app                                          壳
Contents/Resources/node/bin/node                                Node（唯一需要 entitlements）
Contents/Resources/runtimes/<ver>/node_modules/**/*.node        node-pty, koffi, require-builtin,
                                                                node-addon-system, sharp
Contents/Resources/runtimes/<ver>/node_modules/**/rg, *.dylib  ripgrep, libvips
```

顺序是契约：**由内向外**（深的先签，app 最后），因为签名覆盖其内容。
全部使用 `--options runtime --timestamp`。

「11 个」这个数字是脚本自己数出来的，不是我对扩展名的假设——`find … -exec file` 按实际
Mach-O 判定，这样新增一个原生依赖不会静默漏签（漏签的后果是公证在最后一步失败）。

### 19.3 已知未决

- **公证按用户决定跳过**。代价是首次打开需右键 → 打开；`sign-app.sh` 末尾会明确打印这一点。
  加公证是末尾两步（`notarytool submit` + `stapler staple`），不需要重写脚本。
- **slim 变体仍然不可分发**：它靠 PATH 上的 Node，而 PATH 上的 Node 在各机器上不一定存在、
  也可能是不支持 `require-builtin` 的构建。要在别处用，得让用户装 Node，或改用 bundled 变体。
- **`libiconv` 的修法是针对这一个库的**（唯一一个非系统依赖）。若将来新增依赖引入别的
  store 库，`installPhase` 里的断言会失败并打印清单，而不是静默产出跑不起来的包。


## 20. 闭包裁剪：显式策略 + 运行期证据

**背景**：bundled 变体 705 MB = 种子闭包 546 MB + Node 131 MB + pnpm 20 MB + 壳 7.5 MB。闭包由 `pnpm deploy --prod @deepseek-ai/dsh` 决定，所以它带着 harness 发布的**每一个**插件包（496 个包里 249 个是 `@deepseek-ai/dsh-*` 插件，但它们的 JS 加起来只有约 50 MB）；"这次启动实际加载哪些"是运行期由 profile bundles 决定的。

**为什么不能自动最小化**：实测（0.1.6-alpha.2，对闭包内全部 JS 做 `import`/`require` BFS，入口 `@deepseek-ai/dsh` + `dsh-base` + `dsh-web-app`）——**471 个包里只有 54 个静态可达（12.7 MB）**。因为 harness 的 bundle/插件是按名字在运行期解析加载的（`@deepseek-ai/dsh-app-boot` 的模板表），`@deepseek-ai/dsh-web-frontend` 甚至是 `dsh-web-app` 的声明依赖却从不被 import。静态图在这里既会误删功能、又会漏掉依赖，**不能当作裁剪依据**。

**三条腿**：

1. **证据**：`pnpm runtime:trace`（[scripts/trace-runtime.mjs](scripts/trace-runtime.mjs) + [scripts/lib/module-trace.mjs](scripts/lib/module-trace.mjs)）用 `node --import` 注入 `registerHooks`，把 runtime 的每次模块解析写成 `.dsh-build/module-trace.jsonl`，并给出"启动加载了哪些包 / 从未加载哪些包（含大小）"。
   - 子进程仍由 `run-runtime.mjs` 拉起，argv 与环境（`DSH_*`/`npm_*`/`NODE_OPTIONS` 被剥掉）与壳给子进程的一致；`MODULE_TRACE_LOG` 刻意不用 `DSH_*` 名字，否则会被那层过滤掉。
   - 首次实测（`--profile web`，与 app 默认 preset 同一组 bundles）：**1126 次解析、268 个包被加载（74.7 MB）、203 个包从未加载（403.0 MB）**。
   - **"从未加载"不等于"可以删"**：node-pty（终端）、ripgrep（搜索）、sharp（图片）、MCP/ACP、其它厂商 SDK 全在这一列——它们是**懒加载的功能**。报告是证据，不是判决；每条裁剪都要由 `effect` 说明代价。
2. **策略**：[runtime-omit.json](runtime-omit.json)，每条必须写 `name` + `why` + `effect`（去掉后什么功能、在什么时候、报什么错）。`scripts/prepare-runtime.mjs` 把它写成 staging `pnpm-workspace.yaml` 的 `overrides: {'<pkg>': '-'}`，包**根本不进闭包**（连同它的私有依赖一起不下载）。`runtime.json` 记录 `omitted`，诊断因此能说"这个构建没有 X"。
   - 坑：pnpm 11 **不再读** `package.json` 里的 `pnpm.overrides`（只打 WARN），必须写在 `pnpm-workspace.yaml`。
3. **两道门**：
   - **静态引用扫描**：裁完后扫描保留的文件，出现 `from "X"` / `require("X")` / `import("X")` 就构建失败。算出来的说明符（`require.resolve(\`${prefix}-${target}/package.json\`)`）**不算**——肯这么写的就是设计成可选的包，那类由 trace + 冒烟负责。
   - **陈旧条目**：`--full`（不套策略）是唯一还能看见被裁包的构建，所以由它断言"清单里每个名字确实存在"；否则上游改名/移除后，清单会烂掉却照样发字节。裁过的构建反过来断言"确实不在"。
   - 运行期依旧走 `runtime:probe` 与壳的种子冒烟。

**首批条目**：`@deepseek-ai/libreoffice-kit-darwin-arm64`（255 MB）。它是 `@deepseek-ai/libreoffice-kit` 的 optionalDependency，由 `createConverter` 内部 `require.resolve` 惰性解析，闭包里没有第二处引用；代价只有 office→PDF 在用时报 `Required LibreOfficeKit native package is missing`。

## 21. 启动器列出全部 profile（并去掉自动启动）

**背景**：装进 `/Applications` 之后打开壳，列表里只有一行 `Default`。两件事凑在一起：

- 壳列的是 `$DSH_HOME/dsh-app/presets/*.toml`，而 CLI 早就把 profile 建在
  `$DSH_HOME/profiles/`（`web`、`demo`、`au-web`…），一个都不在列表里；
- 壳启动时自动起"排序后的第一个 preset"，于是"打开壳"等于"打开一个 `dsh-app` profile 的窗口"。

用户要的是反过来：**打开壳先看到全部 profile，起哪个由我自己决定**。

### 21.1 发现，而不是让用户再抄一遍

`profiles/<name>/package.json` 的 `dsh.profile` 段本来**就是**一份完整的启动声明：`bundles`
决定它能以哪种载体起，`patchReload` 决定它是否需要 internals（D3.1 的映射表原样适用）。
所以壳读它，而不是要求用户把每个 profile 再写成一个 preset：

- 清单来自 profile 自己的 manifest（`profile::discover`）；
- `kind` 由 bundles 推出：含 `@deepseek-ai/dsh-web-app` → `web`；含
  `@deepseek-ai/dsh-headless` → `task`；含 `@deepseek-ai/dsh-acp-app` /
  `@deepseek-ai/dsh-sdk-app` / `@deepseek-ai/dsh-sdk-minimal` → `server`；
  只有安装层 `dsh-base` → `none`。**四种都列出来**，但只有 `web` 可启动，
  其余的 Start 是 disabled 并写明缺哪个 bundle——这正是 D3.3 要消灭的那类
  "起了一个永远不出 URL 的进程"；
- 发现的 profile **不物化、不写入**：`profile::materialize` 对已有 profile 本来就是
  no-op，而 profile 自己的 `cordis.patch.yml` 是用户层，壳不碰（§11.5 的边界继续成立）；
- `profiles/node_modules`（全 home 的安装回退，上游同样拒绝把它当名字）与没写 manifest 的
  目录不是 profile，跳过；写了 manifest 但**没有** `dsh.profile` 段的目录会报进 problems，
  而不是静默消失。

### 21.2 一份 profile 只有一行

preset 的 `profile` 指向哪个 profile，那个 profile 就不再单独列一行：preset 是更具体的表述
（可能带 `patches` / `env`），两行都能起同一个 profile 只是同一件事的两种说法。
顺序固定为**先 preset（按文件路径）、后 profile（按名字）**，preset 的默认列表位置不变。

### 21.3 不再自动启动

`setup` 里 "起 `presets.first()`" 那段删掉了：

- "排序后第一个"本来就不是语义（§13.2 记的正是这条），而"记住上次选择"会让壳在用户没看列表前
  就拉起一个后端；
- 没有东西在跑是壳的默认状态，用户按下 Start 才产生第一个子进程；
- 未配置 Node 的 `slim` 壳本来就不能自动启动，去掉之后两条路径合一。

### 21.4 实测

| 检查项 | 结果 |
|---|---|
| `cargo test`（27 项，新增 9 项） | 通过：发现/跳过的边界、坏 manifest 报错、profile 去重（含 id 撞名）、排序、kind 分类、以及页面实际消费的 payload 形状 |
| `cargo build` | 与改动前同样 9 条 warning（都是既有的 dead-code） |
| 打包态实测（`sign-app.sh bundled` 的产物 + 一份指向真实 profile 的隔离 home） | 列表 6 行：2 个 preset（`Default`/`Second`）+ `au-web`/`demo`/`desktop`/`web`；`dsh-app` 只出现一次；6 行全部 `idle`、`url` 为空、无任何子进程（确认不再自动启动）；`problems` 为空 |
| 签名与冒烟（同一产物） | 13 个 Mach-O + app 带 `TeamIdentifier=XQYB6GQJJ5`，`--verify --deep --strict` 通过，种子冒烟通过（node / pnpm / web 三件事都没被这次改动碰到） |

## 22. 壳不建 profile，也没有默认 preset（launcher 定位）

§21 让列表能列出用户自己的 profile 之后，"壳自带一个默认 preset、第一次 Start 时再物化一份
自己的 profile"就只剩代价了。

**用户的话**：不需要 default，也不需要生成我们自己的 profile；我们是一个壳，定位是 launcher，
进来能看到用户的 profiles，他自己选要用哪个。

### 22.1 删掉的三样

| 删掉 | 原来在哪 | 为什么 |
|---|---|---|
| 默认 preset（`dsh-app.toml`，label `Default`） | `preset_store::default_preset_text`，每次加载发现缺失就写 | 它让列表有两个来源：用户的 profile，和壳自己造的一行 |
| 示例 preset 的 seed（`second.toml` + `patch.d/`） | `build.rs` 暂存到 `OUT_DIR/seed-presets`，`seed_missing()` 首次运行拷进 home | 壳往用户的 home 里放自己发明的条目，而它们指向并不存在的 profile（`dsh-app-second`），Start 时又把这个 profile 造出来 |
| `preset_seed_dir()` / `seed_missing()` / `default_preset_text()` / build.rs 的 staging | 同上 | 没有调用者了 |

示例仍留在仓库里（`src-tauri/presets/`，附 README）；想用的人自己拷进
`~/.dsh/dsh-app/presets/`。

### 22.2 上游的规则本来就支持这个定位

`@deepseek-ai/dsh-app-boot` 的 `loadProfile`：名字在 `PROFILE_TEMPLATES`
（web / headless / acp / sdk / sdk-minimal）里且 profile 不存在时，**harness 自己**初始化；不在
表里的名字不存在就直接报错——`profile "X" does not exist; create it with 'dsh plugin --profile X add <package>'`。
而唯一被独占的名字是 `desktop`，那是上游保留给 Electron 的（`dsh/lib/bin.js`：
`profile "desktop" is managed exclusively by the Electron application`）。

结论：**profile 的创建权在上游和用户手里，壳不该插一脚**。壳只要 `dsh --profile <name>`
起进程；模板名的 profile 该建的时候 harness 会自己建。

### 22.3 还留着的一个写入口（有意的）

preset 的 `profile` 指向一个**不存在**的 profile 时，第一次 Start 仍会由
`profile::materialize` 建出来（D3 / §11.5 的契约：preset 是用户写的声明，`bundles`
是"首次物化时写什么"）。区别是：现在没有任何 preset 是壳带来的，所以这条路径只在用户自己写了
preset 之后才会走到。要彻底到"壳永不写 `profiles/`"，删掉那次调用即可。

### 22.4 升级注意

装过旧版本的机器上，`~/.dsh/dsh-app/presets/dsh-app.toml`（以及可能存在的
`second.toml`、`patch.d/`）**不会**被删——壳不删用户的文件。不想要就自己删：
`rm ~/.dsh/dsh-app/presets/dsh-app.toml`，之后壳不会再写它。已经建出来的
`profiles/dsh-app/` 就当成一份普通 profile 留在列表里。

### 22.5 实测

| 检查项 | 结果 |
|---|---|
| `cargo test` | 28 项通过（新增 1 项：空 home 不写任何 preset；其余为 §21 的发现/去重/顺序用例） |
| 打包态实测（签名后的 app + 指向真实 profiles 的隔离 home） | 5 行全部 `origin=profile`（au-web / demo / desktop / dsh-app / web），没有任何壳自造的条目；`dsh-app/presets/` 运行前后都是 0 个文件；无子进程（不自动启动）；problems 为空 |

## 附录 A — Tauri v2 平台事实（已核实，2026-09-16）

> 版本基线：tauri 2.11.5 / tauri-runtime-wry 2.11.4（wry 0.56.0）/ tao 0.37.0 / tauri-plugin-updater 2.11.0。
> 下列事实**推翻了本稿早前的两处假设**，标注为 **[改]**。

### A.1 自定义协议不能承载本项目的流量 —— D1 由"偏好"升级为"必须"

**[改]** `register_uri_scheme_protocol` / `register_asynchronous_uri_scheme_protocol` 的签名是 `http::Request<Vec<u8>>` → `http::Response<T: Into<Cow<'static,[u8]>>>`，**请求与响应都全量缓冲**。流式响应 issue 自 2023 年开放未解（wry#1022）；Tauri 官方 streaming 示例是"缓冲 Vec + HTTP Range"。Rust→JS 的流式要用 `tauri::ipc::Channel`。

结论：把 dsh 的 HTTP/SSE/WS 走自定义协议**在 v2 上不可行**（除非用 Channel 重写客户端传输层，那等于重做上游已完成的工作）。**loopback 是本项目的正确解，而不是折中**。这也解释了为什么上游 Electron 用自定义协议能成立：它用的是 Chromium 的 `protocol.handle` + Fetch（真流式），Tauri 没有等价物。

### A.2 安全上下文：`127.0.0.1` 反而比 Tauri 自定义协议更可靠

`http://127.0.0.1:PORT` 是 W3C 定义的 potentially-trustworthy origin，不受 mixed-content 阻断。而自定义 scheme 的 secure-context 状态**平台分裂且 macOS 未文档化**：WebKitGTK 显式 `register_uri_scheme_as_secure`，Windows 走 `*.localhost`，macOS 没有等价物，wry 相关 issue 仍开放。**结论：macOS 上不要依赖自定义协议是 secure context**；loopback 同时解决这个问题。

### A.3 侧车（sidecar）的真实契约

| 事实 | 对设计的影响 |
|---|---|
| v2 shell 插件只在 JS `spawn` 时把子进程登记进 `ChildStore`；**Rust 侧 `sidecar().spawn()` 的子进程不会被自动回收** | 必须自己管生命周期（R1 的处置从"加保险"变成"必须实现"） |
| 插件版 `spawn` 强制新建 stdin/stdout/stderr 管道，**不支持额外 fd 继承** | Electron 的 `stdio:[…,'ipc']` 在 Tauri 无对应物；我们的设计只用 stdout 行 + 信号，正好避开 |
| `kill` 只对直接子进程 SIGKILL，**不做进程树清理** | 用 `std::process::Command` + `process_group(0)` + 自己 kill 进程组（Rust 侧直接拿到 `pre_exec`/`process_group`） |
| 父进程被 SIGKILL 时子进程侧不会收到任何通知 | 父死检测必须放在**子进程**：stdin EOF / `getppid` 变化（Node 侧可用 `process.on('disconnect')` 与 stdin end；我们已用管道 stdin，天然可用） |
| sidecar 路径解析基准是 `current_exe().parent()` ⇒ `Contents/MacOS`，**不是 Resources** | 若走 `externalBin`，Node 要放在 `Contents/MacOS/`，资源放 `Contents/Resources/`（数据） |

### A.4 macOS 打包与签名（本次调研最关键的修正）

**[改]** Tauri 的 macOS bundler **只签 `MacOS` / `Frameworks` / `Plugins` / `Helpers` / `XPCServices` / `Libraries` 这几个目录，且只认 `.framework/.xpc/.app/.dylib/无扩展名` 文件** —— `Contents/Resources` **永远不会被签**，`.node` 扩展名也被跳过。`codesign --deep` 自 macOS 13 起已废弃用于签名，且同样到不了 Resources。

因此 §7.2 的签名清单不是"补充说明"，而是**流水线的核心步骤**：

1. 每个 Mach-O（`node`、`*.node` 含 `node-pty`/`koffi`/`node-addon-require-builtin`/`system.node`、`spawn-helper`、`rg`、任何 `.dylib`）**逐个**用 Developer ID 签，带 `--options runtime --timestamp`。
2. 因为全部用**我们自己的 Team ID** 重签，理论上不需要 `com.apple.security.cs.disable-library-validation`；只有在漏签任何 npm `.node` 时才需要它作为兜底（那意味着清单没穷举，应视为失败而不是加 entitlement）。
3. 之后**不得再修改任何被签文件**；`codesign --verify --deep --strict` + 冷启动冒烟作为断言。
4. **已知风险**：`externalBin` 的公证存在未解决的失败报告（tauri#11992，主二进制签名无效）。因此 **M8 必须先做一次端到端试签**，并在两条实现路线里选一条：
   - 路线 1（确定性高，本稿默认）：`cargo build` → 自己组装 `.app` + 自己逐文件签名 + 自己 `hdiutil`；
   - 路线 2（少写脚本）：`cargo-tauri` 走官方 bundle（自动放 `externalBin` 并签 MacOS 目录），但需接受 tauri#11992 的不确定性。

### A.5 Nix

- 官方支持路径是 nixpkgs 的 **`cargo-tauri.hook`**（25.05 / 25.11 / master 均有）。**nixpkgs 没有 `tauriBundle` / `buildTauriApp` 之类的 helper**（只有第三方方案）。
- `beforeBuildCommand` 由 **tauri-cli** 执行（`helpers::run_hook`，cwd 为前端目录），不在 cargo 链路上 → Nix 里要在其中跑包管理等于引入网络/可写缓存等不确定输入。本设计的 hook 为空，**不执行**。
- `frontendDist` 有**两处**存在性检查：CLI 层 `bail!`，编译期 `generate_context!()` 里 `panic!`，随后 `EmbeddedAssets` 递归读入二进制 → "必须在 Rust 构建前存在"是字面意义上的（它就是构建的一部分）。本设计的 `frontendDist` 是仓库内静态目录，天然满足。
- 将来若要在 Nix 里跑官方 bundle：`postPatch` 剥 hook + `--config`/`TAURI_CONFIG` 注入 store 里的 `frontendDist`。当前设计不需要。
- pnpm 相关属性在 25.05 上不存在，需要 `nixpkgs-unstable`（本仓库 `flake.nix` 已用 `nixpkgs-unstable`，符合）。
- Darwin 默认只出 `app` bundle；DMG 步骤在 headless CI 下需要 `CI=true`。

### A.6 更新器与分发

- `bundle.createUpdaterArtifacts`（bool 或 `"v1Compatible"`，默认 false）**不是 bundle target**；macOS 产物是 `MyApp.app.tar.gz` + `.sig`，minisign/Ed25519，**整包替换无增量**，且需要 app 类型目标才会产出。可用于非 App Store 分发。
- **没有任何官方 Homebrew 工具链**，`latest.json` 里也没有 sha256 → cask 必须自己算 sha256 并在自定义 tap 里维护（与 §6.1 一致）。homebrew-core 里没有任何 tauri formula，**源码 formula 不现实** → 印证 §6.1 的"cask 为主、slim 走预编译 tarball"。

### A.7 仍然未核实（不要当成结论）

- macOS 上 `tauri://localhost` 的 `isSecureContext` 具体取值（本设计不依赖它）。
- wry#1778（macOS 26.6 beta 中跨自定义 scheme 子资源加载失效）是否已进入正式版。
- 内置 Node 是否**仅靠** Tauri 自带公证流程就能通过（这正是 A.4 路线 1/2 之争）。
- `hdiutil` 在 Nix sandbox 内的可用性（M8 若不打算在 Nix 里出 DMG 则无关）。

## 附录 B — 复用的上游配方索引

| 需求 | 上游文件 |
|---|---|
| npm 闭包产出 | `scripts/build-exe-for-python-sdk.ts:289-299` |
| 单文件 exe（未来替代 Node+node_modules） | 同上 + `@yao-pkg/pkg --sea`（`patches/@yao-pkg__pkg@6.21.0.patch`） |
| 闭包校验 | `scripts/verify-runtime-closure.ts` |
| Node 归档下载+校验 | `apps/desktop/scripts/prepare-runtime.ts:14,39-53` |
| 离线安装/种子完整性 | `apps/desktop/scripts/prepare-seed.ts`、`apps/desktop/src/seed-store.ts` |
| macOS Mach-O 重签 | `apps/desktop/scripts/macos-seed-store.ts` |
| 子进程 spawn 配方 | `apps/desktop/src/host-process.ts:99-125` |
| 就绪/退出语义 | `apps/desktop/src/host-process.ts:200-220`、`apps/cli/src/process-shutdown.ts:4` |
| 管道协议（**仅参考，不实现**：Tauri 无法经济承载，见 A.1） | `apps/desktop/src/host-protocol.ts`、`apps/desktop-host/src/wire.ts` |
