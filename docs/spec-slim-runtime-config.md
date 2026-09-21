# Spec：slim 变体的运行时配置

> 状态：**已定稿，待实现**。决策见 §5。实现前必读 §7。

---

## 1. 结论（TL;DR）

`bundled` 与 `slim` **同一个二进制**，差别只在 `Contents/Resources`：`slim` 没有 `node/`、`pnpm/`、`bin/`。

1. **slim 不猜**。用户填 Node（pnpm 选填），不填不报错——"未配置"不是错误状态（这是本 spec 的核心）。
2. **表单由壳页面渲染**，不能由运行时渲染：运行时需要 Node 才能起，未配置时它不存在。
3. **设置存 `<app home>/dsh-app/settings.json`**（app 级，不是 preset 级）。
4. **解析顺序**：`DSH_APP_NODE` > preset > settings > bundle > 未配置。
5. **校验在用户点按钮时发生**，不预先报错。

非目标：不做自动探测（PATH／标准位置都不扫）；不改 `bundled` 的零配置行为；不做 `deferred`（首次下载 Node，见 §8）。

---

## 2. 解析顺序

```
1. DSH_APP_NODE          ← 会话级（终端排障）
2. preset 的 node 字段    ← 按 preset 覆盖
3. settings.json 的 node  ← 表单里填的（机器属性）
4. bundle 内的 Node       ← 只有 bundled 有
5. 都没有 → 【未配置】     ← slim 的默认状态，不是错误
```

**原则：更具体的作用域赢。** 一次运行 > 一种工作方式 > 这台机器 > 这个 app。

- 第 2 级在 settings 之上，是为了让"某个 preset 用官方 Node（需要 `require-builtin`）、另一个用系统 Node"真的可用。反过来则第 2 级永不可达。
- **dev 构建例外**：`cfg!(debug_assertions)` 时仍保留标准位置 + PATH 兜底，否则 `cargo run` 不能用。发布构建绝不猜。
  ⇒ **推论：dev 构建下"未配置"流程不会出现，验证它必须用 `cargo build --release`。**

### 2.1 两个解析上下文（别漏）

| 上下文 | 调用点 | 可用级别 |
|---|---|---|
| **全局** | `config::load`、seed 冒烟、`--update-runtime`、`apply_update` | 1、3、4 |
| **单 preset** | `start_preset` | 1、2、3、4 |

### 2.2 settings.json

```json
{ "schema": 1, "node": "/opt/homebrew/bin/node", "pnpm": "/opt/homebrew/bin/pnpm" }
```

- **写盘纪律**：临时文件 + `rename`；损坏文件按"未配置"处理并显示原因；`schema` 不匹配则拒绝，不静默丢值。
- **位置风险**：开发态仍是 `<repo>/.dsh-build/home`（被 `.gitignore` 忽略），`git clean -xdf` 或删 checkout 会带走它，而里面是用户手输的绝对路径。**打包态已切到 `~/Library/Application Support/dsh-app`**：原来 `repo_root()` 把 `/Applications/dsh-app.app/...` 反推成 `/Applications`，状态会被写进系统目录，标准（非 admin）账号干脆起不来（Q6 已了结）。

---

## 3. 校验

**不预先报错。** 首次进入只显示表单。

| 触发点 | 有没有 runtime 闭包 | 检查什么 |
|---|---|---|
| 点「校验并保存」 | **没有** | 存在且是文件；**可执行位**；能跑 `node --version`（确认是能跑的 Node，**不是**版本判定） |
| 点 Start（未校验过） | **有** | 上面全部 + require-builtin 能力 + 版本区间（**只警告，不拦**） |

| 情况 | 行为 |
|---|---|
| Node 路径无效 | 显示错误，**不保存** |
| pnpm 路径无效 | 同上；**不挡 preset 启动**，只有插件操作报错 |
| 不支持 `require-builtin` | **允许保存与启动**，只警告（`patchReload: live` 会失败） |
| 版本不在 `^22.19.0 \|\| >=24.0.0` | **只警告，不拦**（Q5） |
| 未配置就点 Start | 提示"请先填写 Node 路径"并聚焦表单。**是引导，不是错误弹窗** |

### 3.1 ⚠️ 不能"复用 `node_probe.rs`"做表单校验

`node_probe::probe(node, closure)` 需要**已安装的 runtime 闭包**（它从闭包 `node_modules` 里 require addon）。而"未配置"恰恰是还没有可用 runtime 的状态，表单第一次校验时闭包不存在。另外：

- `NodeCapability.supported` 是 `require_builtin` 的别名，**没有版本判定**；`NODE_REQUIREMENT` 是死代码。
- 它只覆盖 doctor 四类检查中的一类，且更弱（丢弃返回值）。
- 无 `Serialize`、未暴露 IPC、`Command::output()` 无超时（直接在命令里调用会冻住 UI）。
- 缓存键含闭包但失效只看 Node 的 mtime。

**可复用的只有 `resolve_node_with` 的存在性判定**；"可执行位／能跑"是新写的；能力探测只在启动期做。

### 3.2 ⚠️ 未配置时的三个分支（不写就会满屏错误）

| 位置 | 未配置时应为 |
|---|---|
| seed 强制冒烟 | **跳过冒烟与激活，不写 `problems`** |
| `setup` 里的 autostart | **不自动启动** |
| `start_preset` | 发**可辨识**的"未配置"信号，UI 引导到表单 |

另需一条**重试路径**：seed 安装目前只在启动时做一次，保存成功后要能重新安装 + 冒烟，否则用户填完 Node 只看到"没有 runtime"。

### 3.3 IPC 契约

```
validate_runtime_paths(node?, pnpm?) -> { ok, errors[], warnings[], node?, pnpm? }
save_settings(node, pnpm?)           -> 同上报文；校验失败不写盘
start_preset 未配置时                 -> emit { phase: "unconfigured", field: "node" }
shell_state                          -> 增加 configured: bool 与已保存路径
```

`snapshot()` 已有 `node` / `nodeSource` / `bundledPnpm`，表单直接复用，不要新造字段。

### 3.4 已知限制

未配置 ⇒ `--update-runtime` / `apply_update` 不可用（都要 Node 做冒烟）。`--check-update` 不受影响。本次接受，但要在 UI 与 README 写明人工出口：手写 `settings.json`，或 `DSH_APP_NODE=/path node dsh-app --update-runtime`。

---

## 4. pnpm（只影响插件）

`dsh plugin` 从 **PATH** 上找 `pnpm`，不参与 GUI 启动。所以 pnpm 选填。

| 变体 | 实际执行 | 用哪个 Node |
|---|---|---|
| `bundled` | `Resources/bin/pnpm`（wrapper，`flake.nix:292-303`） | **写死** bundle 内 Node |
| `slim` | 用户填的 pnpm（其目录前置到子进程 PATH） | 它自己的 shebang |

⇒ 用户换 Node 后 pnpm 仍跑 bundle 的 Node，这是**真的**，但原因是"wrapper 的启动 Node 固定"，不是"pnpm 只是 JS，谁跑都行"。

---

## 5. 决策记录

| # | 问题 | 决定 |
|---|---|---|
| Q1 | 未配置时 harness 窗口？ | 不打开，只显示表单（没有可加载的 URL） |
| Q2 | pnpm 必填？ | 选填；留空时显示"插件管理不可用" |
| Q3 | bundled 里覆盖 Node 后 pnpm 用谁？ | 各管各的；机制见 §4 |
| Q4 | settings 与 preset 谁优先？ | **preset 覆盖 settings**（更具体的作用域赢） |
| Q5 | 版本不在区间拦不拦？ | **不拦，只警告** |
| Q6 | settings 存哪？ | `<app home>/dsh-app/settings.json`；打包态 app home 是 `~/Library/Application Support/dsh-app`，开发态仍为 `<repo>/.dsh-build/home` |

---

## 6. 已否决

| 方案 | 否决理由 |
|---|---|
| **自动探测**（PATH／标准位置） | Finder 启动的 app 不继承 shell 环境（只有 `/usr/bin:/bin:/usr/sbin:/sbin`），PATH 反映不了用户装了什么；扫标准位置选中的是用户没选过的 Node，与需求矛盾 |
| **`deferred`**（首次下载 Node） | 与离线优先冲突；新增联网失败路径；体积应先裁后下载（`node-pty` 的 win32 prebuild 白占 24MB） |
| **未配置即报错退出** | 把"没填"当成"填错了"，与需求 2 矛盾 |
| **settings 优先于 preset** | 会让 §2 的第 2 级永不可达 |

---

## 7. 当前工作区状态

**改到一半，但能编译。**

| 项 | 实测 |
|---|---|
| `cargo build`（debug） | ✅ 通过（0 error / 11 warning） |
| `cargo build --release` | ✅ 通过（34s） |
| `sign-app.sh` 签名链 | ✅ 已用真实 Developer ID 跑通（设计稿 §19） |
| `withPnpm` 打包 pnpm | ✅ 已实现 |
| `supervisor.rs` 前置 `Resources/bin` | ✅ 已实现（目录不存在时自动跳过，slim 安全） |

**未验证**：preset 的 `node` 是否真的传进解析（接线看起来已接上，未端到端跑过）。

**第一阶段**：跑一次 `cargo build` 确认绿 → 把当前半成品拆成可提交的基线 → 再按本 spec 改。

⚠️ **已删除的错误断言**：旧版说 release 解析依赖 `config::load` 的 `current_dir()/result` 分支。`src-tauri/src/` 里不存在这个分支（`config.rs` 无 `current_dir`），"`nix build --no-link` 必须先建 `result`"的前提不成立。

---

## 8. 待改文件

| 文件 | 改动 |
|---|---|
| `src-tauri/src/settings.rs` | **新增**：读写 + 原子写 + 损坏容错 |
| `config.rs` | 解析链按 §2；`Err` → `NodeResolution::{Configured, Unconfigured}`；`NodeSource` 加 `Settings`；`is_file()` 加可执行位 |
| `lib.rs` | 两个新命令（§3.3）；三个未配置分支（§3.2）；seed 重试路径；payload 加 `configured` |
| `ui/` | 表单 + 校验反馈 + 新 `nodeSource` 变体显示 |
| `supervisor.rs` | 用户填了 pnpm 就用它（优先于 bundle 的 `bin/`） |
| `node_probe.rs` | 本次不动；若表单也要能力探测，需加 `Serialize` DTO + 超时 + 闭包指纹 |

**顺序**：提交半成品基线 → `config.rs` 语义与顺序 → `settings.rs` → IPC + UI → pnpm PATH。
