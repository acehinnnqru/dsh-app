# Spec：外部 dsh（壳作为纯 launcher）

> 状态：**已实现，并在真实 dsh 上跑通**。四个待决项见 §9（均已定），
> 实测记录见 §13 —— 其中 §13.2 记录了一次被推翻的结论。
>
> 相关：[design-tauri-wrapper.md](design-tauri-wrapper.md)（整体设计，D1/D6 的抽象仍然成立）、
> [spec-slim-runtime-config.md](spec-slim-runtime-config.md)（**被本文档整体取代**，见 D11）

---

## 1. 结论（TL;DR）

1. 壳只接受**两个值**：一个 dsh 可执行路径，和一个 harness home（默认 `~/.dsh`）。
   不探测、不下载、不安装、不更新、不回滚。
2. 壳不是运行时的家长，是**启动器**：spawn → 读就绪行 → SIGTERM。
3. 用户没填路径 = **【未配置】**，不是错误（沿用 slim spec 的核心结论，见其 §1）。
4. 壳**不再拥有**：runtime store、通道与更新、种子、冒烟、Node 探测、internals 判定、
   `DSH_HOME`、preset、patch 层、自备 pnpm。
5. **只有一个产物。** `slim` / `bundled` 这条变体轴整个消失 —— 没有内置来源，就不存在
   "带不带 Node"这个问题；连带 master 通道、第二个 cask、运行时 CI、带变体参数的签名脚本一起消失。
6. 唯一的代价：**装完 app 必须自己先有一个 dsh**。这是产品决定，已被明确接受。

一句话：**壳从"运行时的家长"退回成"运行时的启动器"。**

---

## 2. 解析顺序

表单收两个值，解析顺序都是"更具体的作用域赢"：

```
dsh 可执行:
  1. DSH_APP_DSH       ← 会话级（终端排障 / CI）
  2. 配置文件的 dsh     ← 表单里填的（这台机器）
  3. 都没有 → 【未配置】 ← 壳页面渲染表单，不是错误

harness home:
  1. DSH_HOME          ← 会话级；也是 `DSH_HOME=$(mktemp -d) dsh-app` 的隔离开关
  2. 配置文件的 home    ← 表单里填的（这台机器），`~` 会被展开
  3. $HOME/.dsh        ← harness 自己的默认值
```

### 2.1 配置放哪，与它驱动哪个 home 无关

**壳自己的配置在壳自己的目录里**，不在它驱动的 home 里：

    $XDG_CONFIG_HOME/dsh-app/config.toml      （未设 XDG_CONFIG_HOME 时 $HOME/.config/dsh-app/config.toml）

这样一句话就消掉了整类问题：**文件里的值不能决定文件自己在哪**。如果把配置放在"用户配置的那个
home"里，改一次 home 配置就跟着搬走，而壳下次启动是按老位置去找它 —— 直接失忆。

它还带来两个副作用，都是好的：

- `DSH_HOME=$(mktemp -d) dsh-app` 隔离的是 **harness home**，而不会让 app 忘掉你用哪个 dsh；
- 壳完全不需要碰 `~/.dsh`：实测里 `$HOME/.dsh` 在那次运行中**根本没有被创建**。

要连配置一起隔离就是 `XDG_CONFIG_HOME=$(mktemp -d)`。两个开关互不干扰。

**原则：更具体的作用域赢。** 与 slim spec §2 同一条原则，只是把 Node 换成了 dsh。

**不猜。** 不扫 PATH，不扫标准位置，不做候选路径表。理由见 §7 与 §11。

表单里给用户的提示是**一行字**，不是一张表：

```
在终端里跑 `which dsh`，把路径粘到这里
```

用户的 shell 有真实 PATH，这一行等于用零行代码换到比任何启发式都准的探测，而且不会随
包管理器演进而腐烂。

---

## 3. config.toml

```toml
# dsh-app configuration.
#
# dsh    the executable this launcher supervises.
# home   the harness home it drives; default ~/.dsh.
#
# Both values are read on every start.
schema = 1
dsh = "/etc/profiles/per-user/rqc/bin/dsh"
home = "~/.dsh"
```

- 位置：`$XDG_CONFIG_HOME/dsh-app/config.toml`，否则 `$HOME/.config/dsh-app/config.toml`。见 §2.1。
- **TOML 而不是 JSON**：这个文件是给人改的，注释和"少一个引号也读得懂"是真的收益。壳写它时
  会带上头注释；手写的不带 `schema` 也接受。
- `home` **按用户输入原样存**（`~/.dsh` 就存 `~/.dsh`）：展开是解析器的职责，
  存展开结果等于把某台机器的答案冻进文件里。空白/缺省 = 默认，壳不会写一个空字符串进去。
- **写盘纪律**：临时文件 + `rename`；损坏文件按"未配置"处理并显示原因；`schema` 不匹配则拒绝，
  不静默丢值（沿用 slim spec §2.2）。
- **存用户粘的那串路径，不存 realpath。** 打包机上的实际链条是
  `/etc/profiles/per-user/$USER/bin/dsh → home-manager-path/bin/dsh → /nix/store/…-dsh-0.1.6-alpha.2/bin/dsh`。
  存 realpath 时一次 `nix-collect-garbage` 就悬空；存第一段时 `home-manager switch` 之后自动指向新版本。
- 路径失效**如实报错，不静默回退**（沿用 [config.rs](../src-tauri/src/config.rs) 现有策略：
  "a value that was named but is unusable is an error rather than a reason to fall through"）。

---

## 4. 校验

**不预先报错。** 首次进入只显示表单。

| 触发点 | 检查什么 |
|---|---|
| 点「校验并保存」 | dsh：存在且是文件、**可执行位**、能跑通 `dsh --version`。
home：展开 `~` 后必须是绝对路径，且能被创建出来（子进程要拿它当 `DSH_HOME` 和 cwd） |
| 点 Start（未校验过） | 同上 |
| 启动失败 | 把子进程 stderr 尾部交给 UI；**壳不做二次诊断** |

明确**不查**：

- 不查 Node（版本、ABI、原生模块）——那是可执行自己与其 wrapper 的事；
- 不查 `require-builtin` / internals 能力——见 §7；
- 不查它能不能起 web 载体——那正是启动本身要回答的问题。

`--version` 只**记录**，不判定、不设区间、不拦。它唯一的用途是出问题时知道用户用的是哪一个
（见 §8"版本漂移"）。

---

## 5. 启动契约

### 5.1 argv

```
spawn(path, ["--profile", <name>, "--no-open", "--port", "0"])
```

- **没有任何 node flag。** 不注入 `--expose-internals`，不注入任何 `NODE_OPTIONS`。
  node 层旗标属于那个可执行自己的 wrapper（nix 包就是把 `--expose-internals` 写死在 wrapper 里的）。
- launcher flag（`--profile`）在 app flag（`--no-open`/`--port`）之前 —— 上游语法是
  "第一个不认识的 token 起归 app"（design doc §11.5）。
- 契约在外部那份上已核实：`dsh --help`（0.1.6-alpha.2）仍列出 `--profile`，
  `dsh-web-app/lib/index.js:203` 仍打印 `dsh web: <url>`。

### 5.2 env

- 沿用现有策略：`env_clear()` + 白名单（剥掉 `NODE_OPTIONS`、全部 `DSH_*`、`npm_*`/`pnpm_*`/`corepack_*`）。
- **设 `DSH_HOME` = 壳解析出的 home**（§2 的三级），并且先剥掉继承来的 `DSH_HOME`。
  两个理由都是"避免分歧"：harness 的解析顺序是 `DSH_HOME` > `~/.dsh`，所以用户在
  终端里 export 过的 `DSH_HOME` 本来会把子进程指到与窗口所示不同的 home；而壳自己也
  可能就是从 `DSH_HOME` 取到 home 的（见 §2），那子进程必须被告知同一个答案，
  而不是自己再算一遍。

### 5.3 就绪与关停

不变：读 stdout 的 `dsh web: ` 前缀 → 抓 loopback URL；`SIGTERM` → 5s → `SIGKILL`；
父进程负责进程组清理。**这部分是壳唯一不可删的职责。**

---

## 6. 删除清单

### 6.1 整个文件

| 文件 | 理由 |
|---|---|
| `src-tauri/src/runtime_store.rs` | 没有"安装"就没有 store：current/previous/seed/staging 全部失去对象 |
| `src-tauri/src/channel.rs` | 通道、检查更新、下载、原子替换都随 store 消失 |
| `src-tauri/src/node_probe.rs` | 探测的是"壳自己挑的那个 Node"；壳不再挑 Node |
| `src-tauri/src/smoke.rs` | 冒烟是"提交前验证"，而提交这个动作不存在了（详见 §10 Q-记录） |

### 6.2 代码面

| 项 | 出处 |
|---|---|
| `patchReload` 解析与 `needs_internals()` | `preset.rs`、`profile.rs`（`dsh.profile` 段只剩 `bundles`，用于推 `kind`） |
| `expose_internals` 状态与 `internalsViaFlag` 上报 | `lib.rs` 的 `ShellState` 与 snapshot payload |
| `node_args` 从 Launch 到 argv 的整条通路 | `supervisor.rs` 的 `Launch`/`start()`、`lib.rs` 的 `start_preset` |
| Node 解析链与 Node 表单 | `config.rs` 的 `resolve_node`/`NodeResolution`/`NodeOptions`、`settings.rs` |
| `DSH_HOME` 注入 | `supervisor.rs` 的 `child_environment` |
| 自备 pnpm 与 PATH 前置 | `supervisor.rs`、flake 的 pnpm wrapper |
| `runtimeId` 等只在 store 下才有意义的 payload 字段 | `lib.rs` snapshot |

### 6.3 构建面

| 项 | 出处 | 处置 |
|---|---|---|
| `dshRuntime` / `officialNode` / pnpm 打包 / `withSeed` / `withNode` / `withPnpm` | `flake.nix` | 删除 |
| `dsh-app-bundled` / `dsh-app-slim` / `default` 三个 package | `flake.nix` | 收敛为**一个** `dsh-app` |
| 变体参数 `<bundled\|slim>` 与产物名 `dsh-app-$variant.app` | `packaging/sign-app.sh`、`packaging/export-app.sh` | 去掉参数，只剩一条路径 |
| 从 Node 归档抽取 JIT entitlements | `packaging/entitlements/extract-node-entitlements.sh`、`dist/node.entitlements.plist` | 没有内置 Node 就没有可抽取的对象 |
| 变体循环与运行时清单字段（`dshVersion`/`nodeVersion`/`runtimeArchive`/`channel`） | `scripts/package-release.mjs`、`package-runtime.mjs` | 删除 |
| 运行时归档发布工作流（release / master 双通道） | `.github/workflows/runtime.yml` | 删除 |
| `prepare-runtime.mjs`、`probe-runtime.mjs`、`trace-runtime.mjs`、`run-runtime.mjs`、`prepare-node.mjs`、`lib/module-trace.mjs` | `scripts/` | 删除 |
| `runtime-omit.json` 与 design doc §20 的整套闭包裁剪纪律 | 仓库根 | 没有闭包就没有裁剪 |
| 通道文档与发布清单 | `dist/channel-release.json`、`dist/release.json` | 删除 |

**签名清单随之坍缩**：design doc §19 记录的"13 个 Mach-O + Node/pnpm/闭包内每个原生模块都要重签"
降为壳自身一个二进制。这是本方案在流水线上最大的一笔收益。

### 6.4 保留

| 项 | 说明 |
|---|---|
| `supervisor.rs` 的 spawn / 就绪行 / SIGTERM→SIGKILL / 进程组 | 核心职责 |
| profile 发现（列用户的 profile） | design doc §21，只读，不物化 |
| 壳页面（表单 + 状态 + 日志尾部） | 未配置时它必须先于运行时存在 |
| `tauri-plugin-single-instance` | 见 §10 的 D6；这是 UX，不是锁 |

---

## 7. 明确不做（不变量）

1. **不猜路径。** 不扫 PATH，不扫标准位置，不做候选表 —— 上表那种启发式会腐烂，而且选中用户
   没选过的东西（slim spec §6 已就"自动探测"作出否决，本 spec 沿用）。
2. **不注入任何 node 层旗标。** 一旦壳开始替 wrapper 加 `--expose-internals`，它就又把"安装者"
   这个角色请回来了。
3. **不读运行时的插件图。** 不解析 `patchReload`、不判定 internals、不因 profile 是 `live` 而
   改变启动方式。这是 D6 划的边界：壳对运行时的读取面只有四件事，与插件图无关。
4. **不管理 home。** 不写 preset、不物化 profile；只把壳自己解析出的 home 作为
   `DSH_HOME` 告诉子进程 —— 这是"避免分歧"的一行，不是"管理"。壳自己解析 home 时
   `DSH_HOME` 优先于 `$HOME/.dsh`，与 CLI 一致。

---

## 8. 代价与已知风险

| # | 代价 | 说明 |
|---|---|---|
| C1 | **开箱不可用** | 没有 dsh 时只能显示表单。这是本方案唯一真正的代价，且是产品决定而非技术限制。缓解：表单给一行安装/定位提示。 |
| C2 | 更新与回滚不归壳 | 外部 dsh 的更新发生在壳之外（brew/npm/nix），壳既不能拦也不能撤。UI 必须说清"运行时由你的安装管理"，否则用户会以为功能坏了。 |
| C3 | 版本漂移 | 壳只知道 `--version` 的一个字符串。不再有 `runtime.json` 的 `dshVersion`/`sha256`/`shellContract` 可查。 |
| C4 | 不能上 Mac App Store | 已经放弃（design doc D10），此处无新增。 |
| C5 | 环境必须自足 | 子进程走 `env_clear()` + 白名单，所以外部 wrapper 必须自解析路径（绝对路径 shebang 等）。nix 包满足；这是对用户选择的一个真实约束。 |

---

## 9. 待确认（实现前必须回答）

| # | 问题 | 候选 | 影响 |
|---|---|---|---|
| Q1 | 子进程的 `cwd` 用什么？ | (a) home (b) 不设，用壳的 cwd (c) preset 的 project | 上游语义是"启动不写安装目录"，(a)/(b) 都可能对，但需要一次实测确认 |
| Q2 | preset 机制是否保留？ | (a) 保留 —— 解析链留第 2 级 (b) 删除 —— 壳只列 `$DSH_HOME/profiles/` | 决定 `preset_store.rs` 与解析链的长度 |
| Q3 | dev 形态的 home | **dev 与打包态都用同一个 home**：`DSH_HOME` 优先，否则 `$HOME/.dsh`（删除 `DSH_APP_HOME` 与 `.dsh-build/home`）。隔离运行 = `DSH_HOME=$(mktemp -d) dsh-app`。**注意**：第一版实测曾误判"harness 不读 `HOME`"，纠正在 §13 |
| Q4 | 首启引导做到哪一步？ | (a) 只给 `which dsh` 一行字 (b) 再给一条可复制的安装命令 (c) 壳可以代跑那条命令 | (c) 会让壳重新"做一件事"；这一格是 C1 唯一的缓解手段 |

---

## 10. 决策记录（本轮已定）

| # | 问题 | 决定 |
|---|---|---|
| D1 | 运行时形态 | **只要"外部可执行"（形态 B）**，不做外部闭包目录，不做 node+entry 两字段 |
| D2 | 自动探测 | **不做。** 用户手动指定；表单给 `which dsh` 提示 |
| D3 | `DSH_HOME` | **表单也收这个值**：`DSH_HOME` > 表单 > `$HOME/.dsh`，子进程被显式告知同一个值，继承值先剥掉。壳自己的配置在 `~/.config/dsh-app/config.toml`，与它驱动的 home 无关（§2.1） |
| D4 | preset | 壳不建、不写、不带默认 preset（design doc §22 已定案）；`~/.dsh/dsh-app/presets/` 不再被使用 |
| D5 | `--patch` | 不需要。它是用户的扩展机制（`preset.patches` → 透传路径），纯 B 下壳没有 presets，故不存在 |
| D6 | 单实例锁 | **上游已覆盖**：`dsh-session-persistence-jsonl` 用 `@deepseek-ai/node-addon-system/flock` 对每个 session 的 artifact 目录取非阻塞 `flock(2)`，争用映射为 `SessionAlreadyOwnedError`，进程死亡由内核释放。壳只保留 `tauri-plugin-single-instance` 的 UI 语义 |
| D7 | pnpm | 不关心。`dsh plugin` 找不到 pnpm 时 exit 127，错误自解释；nix 包的 wrapper 自己前置了 pnpm |
| D8 | internals / `patchReload` | **壳完全不参与。** 既不注入旗标，也不判定、不上报。失败时 harness 自己抛 `--expose-internals is required`，这句话本身就是修法 |
| D9 | **变体轴** | **整个消失。** 没有 slim，也没有 bundled —— 壳不带 Node、不带 pnpm、不带闭包，三者都不带，于是"带不带"这个问题没有被提出的场合。产物只有一个，名字就叫 `dsh-app` |
| D10 | 分发与通道 | 两个 cask（`dsh-app` / `dsh-app@master`）收敛为一个 `dsh-app`；master 通道随之消失（它存在的唯一理由是"从 harness main 构建的**运行时**"）。`--check-update` / `--update-runtime` / `--rollback` 三个 CLI 能力，以及 design doc D8/D9/D10 的"运行时独立更新（B）+ 壳走 cask（A）"双轨模型，全部作废，只剩"壳走 cask" |
| D11 | `spec-slim-runtime-config.md` | **整体作废**，不只是 §2–§4。它的前提是"存在 slim 变体"；该变体不存在之后，整份文档没有对象 |

### 关于 D9 的补充：为什么"没有 slim"比"多一个变体"更彻底

变体轴是 `withNode × withPnpm × withSeed` 三个开关。纯 B 下三者同时为假，轴塌缩成一个点 ——
而一个点不需要名字。凡是"为了服务这条轴"而存在的东西一并消失：签名脚本的变体参数、
抽取 Node entitlements 的脚本、发布清单里的 `nodeVersion`/`runtimeArchive`、双通道 CI、
`AGENTS.md` 里"dev 构建探 PATH 找 node"与"slim 变体不猜路径"两条注记（`NodeOptions::Discover`
与 dev/release 行为分叉随之消失）。

### 关于 D8 的补充论证

壳里的整条 internals 机器（`node_probe` + `needs_internals` + `node_args` +
`expose_internals` 状态 + `internalsViaFlag` 上报）当初存在，是因为**壳是运行时安装者**：
它自己挑 Node，所以必须验证那个 Node 的 addon 能力；又因为它自带 preset（D4 默认
`patch_reload = "startup"`），所以需要一个地方表达 `live` 的代价。

这两个前提现在都没了：Node 归 wrapper，preset 归用户（§22）。于是这个读取**没有主人**，
而且它会开始说谎 —— `internalsViaFlag` 报的是"**壳**有没有加这个 flag"，纯 B 下壳永远不加，
于是它对一个实际有 internals 的子进程报 `false`。

实测对照：本机 `~/.dsh/profiles/` 下 `web`/`demo`/`au-web` 全部是 `patchReload: "live"`，
而它们一直都能起 —— 因为 wrapper 自带 `--expose-internals`。壳的这一整套，
是在同一棵进程树的上游重复实现 wrapper 已经做完的事。

**诊断之所以存在，是因为它能驱动一个动作；动作没了，诊断就只是装饰** —— 而且是 D6 明令
禁止的那种装饰。

---

## 11. 已否决

| 方案 | 否决理由 |
|---|---|
| 候选绝对路径表（`/opt/homebrew/bin/dsh`、`~/.nix-profile/bin/dsh`、`/etc/profiles/per-user/$USER/bin/dsh` …） | 表会随包管理器演进而腐烂；nix 世代会留下悬空项；而且用户的 `which dsh` 本来就更准 |
| 静默自动选择（探测到什么就用什么） | 等于启动一个用户没选过的后端；"发现"与"选择"必须分开 |
| 自行扫描 PATH | Finder 启动的 app 看不到用户 shell 的 PATH（只有 `/usr/bin:/bin:/usr/sbin:/sbin`），实测 `command -v dsh` 返回空 |
| 外部**闭包目录**形态（指定一个含 `lib/bin.js` + `node_modules` 的目录） | 用户明确只要可执行形态；闭包目录会把 entry/Node 解析重新拉回来 |
| 用 `--patch` 禁用 HMR 行以绕开 internals | 属于"壳对 profile 内容有意见"，与 D8 冲突；且需要壳持续跟踪上游插件 id |
| 保留一个内置来源（哪怕只是个"下载 dsh"按钮） | 会把 store 以缩小版请回来；开箱代价已被接受 |

---

## 12. 待改文件（实现顺序）

| 顺序 | 文件 | 改动 |
|---|---|---|
| 1 | `src-tauri/src/settings.rs` | 改为 `$XDG_CONFIG_HOME/dsh-app/config.toml`（TOML），字段 `dsh` + `home`；校验改为"可执行 + `--version`"、"home 可创建" |
| 2 | `src-tauri/src/config.rs` | 删 `resolve_node`/`NodeResolution`/`NodeOptions`/`bundled_node_candidates`；新增 `resolve_dsh`；`RuntimeConfig` 由"目录"改为"可执行路径" |
| 3 | `src-tauri/src/supervisor.rs` | `Launch` 改 `exe + args + home`；删 `node_args` 与 pnpm PATH 前置；环境里显式设 `DSH_HOME` |
| 4 | `src-tauri/src/lib.rs` | 删 seed/更新/回滚命令与启动分支；改 `start_preset` 的 argv；删 `expose_internals` 与相关 payload 字段 |
| 5 | `src-tauri/src/preset.rs`、`profile.rs` | 删 `patchReload`/`needs_internals`；`dsh.profile` 只读 `bundles` |
| 6 | 删除 `runtime_store.rs`、`channel.rs`、`node_probe.rs`、`smoke.rs` | 连同其测试 |
| 7 | `ui/` | 表单改为两个字段（dsh + harness home）；`nodeSource` → `dshSource`；未配置文案 |
| 8 | `flake.nix` | 删 `dshRuntime`/`officialNode`/pnpm 打包；`mkApp` 收敛成唯一的 `dsh-app`；删 `dsh-app-slim`/`dsh-app-bundled`/`default` 三个 package |
| 9 | `scripts/` | 删 6 个 runtime 脚本 + `prepare-node.mjs`；`package-release.mjs` 去掉变体循环与运行时清单字段 |
| 10 | `src-tauri/packaging/` | `sign-app.sh`/`export-app.sh` 去掉 `<bundled\|slim>` 参数与 `dsh-app-$variant.app` 命名；删 `entitlements/extract-node-entitlements.sh` |
| 11 | `.github/workflows/runtime.yml` | 删除（没有可发布的运行时） |
| 12 | `package.json`、`runtime-omit.json`、`dist/` 清单 | 删 `dshRuntime` 段、裁剪清单、`channel-release.json`/`release.json` |
| 13 | `AGENTS.md`（仓库根） | 删"dev 构建探 PATH 找 node"与"slim 变体不猜路径"两条注记 |
| 14 | `spec-slim-runtime-config.md` | 标注"被本文档取代"（或删除） |

**验证**：`cargo build`（基线是绿的）→ 用本机 `/etc/profiles/per-user/$USER/bin/dsh`
端到端起一个 `web` profile，走完"未配置 → 填路径 → 校验保存 → Start → 出 URL"。

⚠️ 不再需要 `cargo build --release` 才能验证未配置流程：没有内置来源之后，dev 与发布构建
不再有行为分叉（`NodeOptions::Discover` 消失），未配置状态在两者里都直接可见。dev 的便利出口是
解析链第 1 级：`DSH_APP_DSH=$(which dsh) cargo run`。

---

## 13. 实测记录（实现当天）

### 13.1 启动器契约成立

用壳会用的那条 argv 对着本机真实 dsh（0.1.6-alpha.2，nix 包）在隔离 home 里跑：

    dsh --profile web --no-open --port 0

得到 `dsh web: http://127.0.0.1:55554/?token=…`，并且 harness 自己初始化了 `profiles/web`。
⇒ §5 的契约（launcher flag 在前、app flag 在后、stdout 的就绪行）在外部可执行形态上原样成立。

### 13.2 ⚠️ 一次误判：`HOME` 与 `DSH_HOME` 的真实优先级

第一轮隔离实验用 `HOME=$(mktemp -d)`，子进程却去写 `/Users/rqc/.dsh`，于是本文档的
初版记下"harness 不读 `HOME`，从用户数据库取家目录"。**这条是错的，已订正。**

A/B 复测（同一台机器、同一个 dsh）：

| 命令 | 结果 |
|---|---|
| `env -u DSH_HOME HOME=$tmp dsh …` | ✅ 隔离成功，写进 `$tmp/.dsh` |
| `HOME=$tmp dsh …`（`DSH_HOME` 保持继承值） | ❌ 写进 `/Users/rqc/.dsh` |

Node 侧的判别同样直接：

    HOME=/tmp/definitely-not-my-home node -p '…'
      os.homedir()          => /tmp/definitely-not-my-home   ← 听 HOME
      os.userInfo().homedir => /Users/rqc                    ← 不听 HOME

harness 的解析器（`@deepseek-ai/dsh-home-paths`：`join(homedir(), ".dsh")`）用的是
`node:os` 的 `homedir()`，所以它**读** `HOME`；真正的顺序是
**`DSH_HOME` > `~/.dsh`**。

第一轮之所以失败，是因为**跑实验的那个进程本身就在一个 dsh 会话里，环境里带着
`DSH_HOME=/Users/rqc/.dsh`** —— 子进程继承了它，而它压过 `HOME`。

> 教训：拿"我自己所在的 harness 会话"当实验环境时，`DSH_*` 是会被继承的。
> 隔离实验必须显式清掉或覆盖它，否则测的是自己的环境，不是产品行为。

### 13.3 由此定下的两条不变式

1. 壳驱动的 home = `DSH_HOME`（非空时）> 表单 > `$HOME/.dsh` —— 与 CLI 一致，
   `DSH_HOME=$(mktemp -d) dsh-app` 即隔离运行。壳自己的配置在 `~/.config/dsh-app/config.toml`，不受它影响（§2.1）。
2. 传给子进程的环境：继承的 `DSH_*` 一律先剥掉，然后显式设 `DSH_HOME` = 壳解析出的值。
   于是"窗口列出的 profile"与"子进程写入的 home"不可能指向两个目录。

### 13.4 端到端，以及一个坑

隔离 home + 真实 dsh：壳打印 `1 profile(s) listed`，`shell_state` 返回
`configured: true` / `dshSource: "settings"` / 一条 `kind: "web"` 的 profile，
注入的 page-probe 报 `error: null`。

隔离形态已按 §13.3 复验：`DSH_HOME=$(mktemp -d) dsh-app`（**不动 `HOME`**）下，
壳的 `home` 就是那个临时目录，列出并只列出其中的 `web`。

配置与它驱动的 home 无关这件事也复验过（`env -u DSH_HOME HOME=$fake XDG_CONFIG_HOME=$cfg`，
配置里写 `home = $work`）：

    settingsFile = $cfg/dsh-app/config.toml   ← 壳自己的目录
    home         = $work                      ← 配置决定，profile 就是从这里列出来的
    1 profile(s) listed, configured: true, page error: null
    $fake/.dsh 根本没有被创建                  ← 壳不碰它不驱动的 home

### 13.5 Start 按钮点不动：两个 bug，以及"必须能自己按按钮"的教训

上面那句"没被覆盖的一格"是错的态度 —— 用户第一次手动测试时 Start 毫无反应，而这条路径里
藏着**两个**独立 bug，都是这次实现引入的：

| # | bug | 症状 |
|---|---|---|
| 1 | 页面 `invoke('start_profile')`，Rust 注册的是 `start_profile_command` | 命令不存在 → promise reject → 点击处理没有 catch → **完全静默** |
| 2 | `spawn_profile` 里写成 `resolve_global_dsh(&config.home)`，把**驱动的 home** 当成**壳的配置目录** | `config.toml` 读不到 → 解析成 Unconfigured → 走一个**没有日志**的分支返回 → 照样静默 |

bug 2 尤其阴：`snapshot()` 用的是正确的目录（所以界面显示"已配置"、列表正常），只有启动
路径用错字段 —— 界面说"我准备好了"，点下去什么也不发生。

处置：命令名不再有 `_command` 后缀；页面用 `act()` 包住所有 host 调用，reject 一律显示出来；
新增两个测试 —— `every_command_the_page_invokes_is_registered`（扫 `#[tauri::command]` 声明，
不维护第二份名单）与 `a_start_resolves_the_executable_from_the_config_directory`（钉死字段选择）。

### 13.6 自测开关：`DSH_APP_SELFTEST`

教训是**这个壳没有终端，手点不能作为唯一的验证手段**。所以 `DSH_APP_DEBUG_STATE=1` 的
page-probe 扩展了一个开关：

    DSH_APP_DEBUG_STATE=1 DSH_APP_SELFTEST=web ./dsh-app

它让页面自己去 invoke `start_profile`，然后轮询 `shell_state` 直到 ready/failed，把
结果通过 `ui_probe` 打回 stderr：

    [dsh-app] starting profile web with /etc/profiles/per-user/rqc/bin/dsh (settings)
    [dsh:web] dsh web: http://127.0.0.1:59554/?token=…
    page-probe {"selftest":"invoked","phase":"ready","url":"http://127.0.0.1:59554/?token=…"}

bug 2 就是被它抓出来的 —— 手工点按钮时，`selftest: "invoked"` + `phase: "still-starting"`
这一组合直接指向"命令返回值正常但线程里静默返回了"。命令行下的 GUI，这条通道比截图有用。

### 13.7 坑

`cargo test` **不重建** `target/debug/dsh-app`。改完 `ui/` 或 `lib.rs` 只跑测试、
直接运行旧二进制，看到的还是上一版行为（本文件 §13.4 记过一次，实现过程中又踩了一次）。
改完必须 `cargo build`，或者干脆 `cargo build && cargo test` 一起跑。
