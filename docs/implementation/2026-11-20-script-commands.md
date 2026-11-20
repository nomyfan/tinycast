# Script Commands 设计

## Problem
把 `~/.config/tinycast/scripts/` 里用户自己的脚本列进启动器，回车运行，结果用 HUD 或 popup 里的输出页显示。
需求：`../requirement/2026-11-20-script-commands.md`

## Scope
In：脚本发现与头部声明（name / icon / mode）、两种输出模式（silent、output）、HUD、`.env`、托盘菜单 "Open Scripts Folder…"。
Out：Raycast 格式、compact / inline、参数、多目录、超时、复制输出（以后再加）。

## Assumptions
- 脚本不多（几十个以内），目录在本机磁盘上，扫描一次是毫秒级。
- 脚本不需要 stdin 和终端（TTY）；需要交互输入的脚本不支持。
- 输出按 UTF-8 读，不是 UTF-8 的字节用 `�` 代替。
- 一个脚本可以同时跑多次（silent 连按两次就是两个进程），不做去重。

## Validation Findings
- **HUD 用什么窗口？** 读 nsh-core 源码：`WindowKind::Overlay` 正合适（不抢焦点、鼠标穿透），但要在 `nsh.json` 声明 `capabilities.windows.overlay`；而一旦声明 `capabilities`，没列出的权限（剪贴板、open_url、home_dir…）全部收回，见 `access.rs` 的测试 "Declared without `windows`: none"。
  → 改用 `WindowKind::Popup` + `focusable: false` + `activate: false`。macOS 后端的 `NshPanel` 按 `focusable` 决定能否成为 key window（`nsh-platform-macos/src/window.rs:233`）。
- **popup 关了之后谁来弹 HUD？** `spawn` 的任务属于当前 owner，popup 的 owner 关闭时任务被取消。nsh 的 `Owner` 是 `Copy`，有 `Owner::current()` 和 `Owner::run(f)`。
  → App 启动时把自己的 owner 存进 `Launcher`，silent 运行在 `owner.run(|| spawn(..))` 下。
- **窗口能按内容自动调大小吗？** `window()` 没有这个选项，只有 `default_size` / `default_position(At | Center)`。
  → HUD 用固定大小的透明窗口，大小贴合胶囊，文字超长时截断。
- **同一个 id 能关掉再开吗？** `open_window` 发现同 id 的窗口还在（包括已开但还没渲染的），就把旧的拿来 `focus()` 并返回，不会新开（`nsh-core/src/element/desktop.rs:822-832`）。`close()` 后马上用同 id 再开，可能只是把正在关闭的旧 HUD focus 了一下。
  → HUD 窗口只开一个，id 固定为 `"hud"`。已经在显示时只更新它的文字 signal，用 `set_bounds`（只给 x、y）移到鼠标当前所在的显示器，并重新开始计时，不重开窗口。
- **Popup 的默认属性。** `WindowKind::Popup` 默认 `close_on_escape`、`close_on_blur`、`shadow` 都是 true，层级是 `Floating`；没声明 capabilities 时 `AboveDock` 也允许。
  → HUD 显式关掉这三个，层级用 `AboveDock`，不会被全屏窗口或浮动窗口盖住。
- **合并 stdout 和 stderr。** Rust 1.87 起有 `std::io::pipe()`，项目锁定 1.100：一根管道的写端 `try_clone` 后同时给 stdout 和 stderr，输出顺序和终端里一致，不用加依赖。

## Alternatives Considered
1. **声明写在哪**：脚本头部注释（选用） vs `config.toml` 里 `[scripts.xxx]`。需求要求写在脚本里，而且信息和脚本放在一起，改名、删除不会不同步。
   语法选 `# @name 值`，不用 `# name: 值`：后者在普通注释里很常见，容易误认。只认文件开头的注释块：跳过 `#!` 行和空行，遇到第一行既不是空行也不是注释的代码就停，之后的 `@name` 都不算。`#!` 和 `# @name` 之间常有空行，所以空行不能让解析停下。
2. **怎么发现新脚本**：每次打开 popup 时在后台重新扫描（选用） vs 像 config 一样每秒轮询 vs FSEvents。打开时扫描最简单，看到的总是最新的，平时不占资源；轮询和 FSEvents 只多了"popup 开着时实时出现"，没有实际用处。代价是列表变化时，打开后会有一瞬间显示旧列表。
3. **HUD**：`Popup` + `focusable: false`（选用） vs `Overlay`（要声明 capabilities，见上） vs 系统通知（要通知权限，而且不是 HUD 的样子）。代价是 HUD 窗口不会让鼠标穿透，但它只显示 2 秒左右。
4. **怎么运行**：直接 exec 文件，由 `#!` 决定解释器（选用） vs Tinycast 自己读 `#!` 或用 `/bin/sh` 运行。直接 exec 支持任何语言和编译好的程序，行为和在终端里运行 `./script` 一样；代价是必须 `chmod +x`。所以列表只列有执行权限的文件，这也正好把放在同一目录的 PNG/SVG 图标排除在外。
5. **output 模式下离开页面**：杀掉脚本（选用） vs 让它跑完。离开后输出没处显示，留着它运行只会让用户以为卡住的脚本还在占资源。

## Components
- **`scripts`**（`src/scripts.rs`）：脚本目录的扫描、头部解析，以及运行（启动进程、合并输出、去掉 ANSI 转义、结束时汇报结果、被丢弃时杀掉整个进程组）。两种模式共用同一个运行接口。
- **`dotenv`**（`src/dotenv.rs`）：把 `.env` 文本解析成"要给脚本设置的变量"，处理引号和 `$VAR` 展开；纯函数，不读文件。
- **`config`**（已有，扩展）：顺带读 `~/.config/tinycast/.env`（没有就写一个带注释示例的），把结果和问题放进 `Loaded`，文件变化时和 `config.toml` 一起重新加载。
- **`hud`**（`src/hud.rs`）：在鼠标所在的显示器下方显示一行提示，约 2 秒后关闭。只有一个 HUD 窗口：新提示到来时如果它还开着，就换掉文字、移到鼠标当前所在的显示器、取消旧计时器、重新计时。
- **`script_view`**（`src/script_view.rs`）：popup 里的输出页：返回按钮、脚本名、状态（运行中 / 完成 / 失败），下面是可滚动的等宽输出，运行时自动滚到底。进入页面时启动脚本，离开时脚本随之被杀。
  - 页面里没有 `Command`，所以自己用 `on_key_with` 绑定 Escape（返回列表）：popup 关掉了 `close_on_escape`，不绑定的话 Escape 没反应。返回列表后搜索框要重新获得焦点。
  - 输出每 40 ms 左右批量写进 signal 一次（把这段时间收到的行一起取出），不是每行写一次，避免 `seq 5000` 这种输出让界面卡住。
- **`lib.rs`**（已有，扩展）：列表里加 "Scripts" 分组、行图标、回车分发；`Mode::Output`；托盘菜单项。

## Interfaces

```rust
// scripts.rs
pub struct Script {
    pub id: String,         // "script:<文件名>"，列表和最近使用记录用它
    pub path: PathBuf,
    pub name: String,       // @name，没写就是去掉扩展名的文件名
    pub icon: ScriptIcon,
    pub mode: OutputMode,   // @mode，没写就是 Silent
}
pub enum ScriptIcon { Default, Emoji(String), Image(String /* 文件路径或 data: URI */) }
pub enum OutputMode { Silent, Output }

/// 脚本目录，`~/.config/tinycast/scripts`。
pub fn dir() -> PathBuf;

/// 目录里的脚本，按 name 排序；目录不存在就先建好，返回空列表。
/// 只列有执行权限的普通文件（跟随符号链接，方便把别处的脚本链接进来），
/// 跳过 `.` 开头的隐藏文件和子目录。会阻塞（读文件），在 `blocking` 里调用。读不了的文件跳过。
pub fn scan() -> Vec<Script>;

/// 启动 `script`，环境是 Tinycast 自己的环境加上 `env`，工作目录是脚本目录。
/// 输出（stdout 和 stderr 按出现顺序，已去掉 ANSI 转义）逐行从 `lines` 送出；
/// `done` 在脚本结束时给出结果。启动失败（没有执行权限、没有 `#!`）也从 `done` 给出，
/// 不会出错返回。丢弃 `Run` 会杀掉脚本以及它启动的子进程。
pub fn start(script: &Script, env: &[(String, String)]) -> Run;
pub struct Run {
    pub lines: tokio::sync::mpsc::UnboundedReceiver<String>,
    pub done: tokio::sync::oneshot::Receiver<Outcome>,
    /* 私有：杀进程组的守卫 */
}
/// 失败原因：退出码非 0 是 "Exit code 1"，被信号结束（`code()` 为 `None`）是
/// "Killed by signal 9"，启动失败如 "Not executable: chmod +x foo.sh"。
pub enum Outcome { Succeeded, Failed(String) }
```

silent 模式下 HUD 显示什么（在 `lib.rs` 里由最后一个非空行和 `Outcome` 拼出来）：

| 结果 | 有输出 | 没有输出 |
|---|---|---|
| 成功 | 最后一行 | `✓ <脚本名>` |
| 失败 | `<原因>: <最后一行>`，失败样式 | `<脚本名>: <原因>`，失败样式 |

```rust
// dotenv.rs
/// `.env` 的变量，按出现顺序。展开 `$VAR` 时先查前面行定义的，再查 `base`
/// （调用方给的是：内置 PATH，然后 Tinycast 自己的环境变量，所以 `$HOME` 能用）；
/// 都没有就展开成空。坏掉的行跳过，并说明行号和原因。
/// 写法：`KEY=value`，可加 `export ` 前缀，`#` 开头的行是注释；
/// 无引号和双引号的值会展开 `$VAR` / `${VAR}`（`\$` 是字面的 `$`），单引号的值原样保留；
/// 无引号的值去掉首尾空白，` #` 之后是注释。不支持跨行的值。
pub fn parse(text: &str, base: impl Fn(&str) -> Option<String>) -> (Vec<(String, String)>, Vec<String>);
```

```rust
// config.rs：Loaded 多一个字段
pub struct Loaded {
    pub config: Config,
    /// 给脚本设置的变量：总有 PATH（`/usr/local/bin:$HOME/.local/bin:` 加 Tinycast 自己的 PATH，`$HOME` 已展开），
    /// 再加上 `.env` 的（会覆盖）。
    pub env: Vec<(String, String)>,
    pub problems: Vec<String>,  // 也包括 .env 的问题，形如 ".env line 3: …"
}
pub fn env_path() -> PathBuf;  // ~/.config/tinycast/.env
```
`config::parse` 保持只处理 TOML 的纯函数（现有测试 `parse(TEMPLATE) == Loaded::default()` 不变）；读 `.env` 并合并进 `Loaded` 放在 `config::load()` 里。`config::watch` 同时比较两个文件的修改时间。

```rust
// hud.rs
/// 显示 `message`（只显示一行，过长截断），`failed` 时带失败的样式；约 2 秒后消失。
/// 正在显示时换成新提示并重新计时，旧提示的计时不会把新提示提前关掉。
/// 要在 App 的 owner 下调用（窗口和定时器归它）。
pub fn show(message: &str, failed: bool);
```

```rust
// script_view.rs
/// 运行 `script` 并显示它的输出；离开这个页面（返回、关闭 popup）时脚本被杀。
pub fn script_view(launcher: Launcher, script: Script) -> impl IntoElement;
```

**信任边界**：
- `scripts::scan` 处理用户写的头部：name 去掉首尾空白，空的就用文件名；icon 以 `.png` / `.svg` / `.jpg` / `.jpeg` 结尾的当作文件（相对脚本目录，支持 `~/`），文件不存在就用默认图标，SVG 读成 data URI；其他值当 emoji / 文字；未知的 `@mode` 当作 silent 并打日志。之后的代码不再检查这些字段。
- `dotenv::parse` 处理 `.env`：变量名必须是 `[A-Za-z_][A-Za-z0-9_]*`。
- `scripts::start` 处理脚本输出：按字节读，先按 `\n` 拼出整行再用 `String::from_utf8_lossy` 转成文字（中文、emoji 跨在两次 `read` 之间也不会被切坏）；一行最多收 64 KB，超出的部分丢掉直到下一个换行（防止没有换行的大块输出把内存吃光）；然后去掉 ANSI 转义和除 `\t` 以外的控制字符（保留制表符，`ps`、`lsof` 的对齐输出才不会挤在一起），单行截断到 1000 字符。`poll` / `read` 遇到 `EINTR` 重试。

## Data Model
- `Launcher.scripts: Signal<Vec<Script>>`：App 启动时扫描一次，每次打开 popup 时在后台再扫描，有变化才 `set`。
- `Launcher.script: Stored<Option<Script>>`：output 模式下要运行的脚本；`Mode::Output` 时 `script_view` 读取它。进入输出页时 `nested` 置为 true，返回规则和剪贴板页一样。
- `Launcher.owner: Owner`：App 的 owner，silent 运行和 HUD 挂在它下面。
- 输出页的状态（行、运行状态）是 `script_view` 自己的 signal，不共享。输出最多保留最后 5000 行。
- `.env` 的结果在 `Launcher.settings`（`config::Loaded.env`）里，运行脚本时取当时的值，所以改了 `.env` 下次运行就生效。
- 没有新的磁盘数据；最近使用照常按 `script:<文件名>` 记在 `recent.json`。

## Load-Bearing Decisions
- **一个 `start` → `Run` 接口服务两种模式**：silent 只取最后一个非空行，output 逐行显示。用 `Drop` 来杀进程，这样"离开页面就停"不用单独写取消逻辑：任务随 owner 取消时 `Run` 被丢弃。代价是 silent 模式下的 `Run` 必须挂在 App 的 owner 下，否则 popup 一关就被杀。
- **进程组**：脚本用 `process_group(0)` 启动，杀的时候用 `kill(-pgid, SIGKILL)`（加 `libc` 依赖），连同 `sleep`、`curl` 之类的子进程一起结束。
- **读输出不能无限阻塞**：脚本放到后台的进程（`foo &`）会一直占着管道的写端，阻塞的 `read` 永远等不到结束，读线程就泄漏了。所以读线程用 `libc::poll` 带超时等数据：脚本退出前一直读；退出后最多再读 200 ms，然后关掉读端、结束线程，并给出 `done`。等待超时也能让读线程定期检查 `Run` 是否已被丢弃。
- **杀进程组的时机**：`Run` 被丢弃时，不管脚本是否已退出都对进程组发 `SIGKILL`，这样离开输出页也会清掉 `foo &` 留下的进程。silent 模式在给出 `done` 之后才丢弃 `Run`，后台进程同样被清掉。要长期在后台运行的程序应该用 `open`（或 `launchctl`）启动，它们由系统启动，不在脚本的进程组里；macOS 没有 `setsid` 命令，`nohup` 也挡不住 `SIGKILL`。这一点写进脚本目录的 README。进程组 id 被系统复用要求组内所有进程都已退出且 pid 刚好轮回，忽略这种情况。
- **只列有执行权限的文件**：没 `chmod +x` 的脚本不会出现在列表里。这一点写进脚本目录里的 `README`。Tinycast 第一次建目录时写入它，里面有格式说明和示例（没有执行权限，所以不会被列出来）。
- **PATH 的顺序**：`/usr/local/bin:$HOME/.local/bin:<继承的 PATH>`，然后 `.env` 可以整个覆盖。`$HOME/.local/bin` 用 `HOME` 环境变量拼成绝对路径：exec 按 PATH 找程序时不会展开 `~`。

## Risks / Open Questions
- **HUD 窗口会不会抢焦点或闪一下**（最大风险）：`Popup` + `focusable: false` 是按源码推断的，没实际跑过。坐标已核对：macOS 后端把 `WindowPosition::At` 当作以主显示器左上角为原点的全局坐标，设的是窗口左上角（`nsh-platform-macos/src/window.rs:1569-1572`），和 `displays()` 一致；仍要在多显示器上看一眼。先做这一步，请你在真机上看一下。
- 5000 行放在一个 `text` 里，布局可能慢（已经批量刷新，只是减少了次数）。慢的话改用 nsh 的 `list`（虚拟列表）一行一个。
- 打开 popup 时扫描：如果目录里有很大的非脚本文件，只读每个文件的前 4 KB 解析头部，所以影响不大。

## Implementation Roadmap
- [ ] [risk validation] `hud.rs`（没加临时菜单项，直接用 silent 脚本验证）
      Purpose：确认 Popup + `focusable: false` 不抢焦点、位置正确、2 秒后消失、连续触发时换文字而不是重开
      Verification：在别的 app 里打字时触发，输入不中断；HUD 出现在鼠标所在显示器的下方；1 秒内连点两次，第二条提示完整显示 2 秒；在另一块显示器上再触发，HUD 跟过去（需要你手动看，跑之前我会先问你）
- [x] [core logic] `dotenv.rs` + `config.rs` 读 `.env`
      Purpose：环境变量可配置，问题出现在托盘
      Verification：单元测试覆盖单引号 / 双引号 / 无引号、`export`、`$VAR` / `${VAR}` / `\$`、前面行的引用、`$HOME` 取自 base、行尾注释、坏行；`parse(TEMPLATE)` 的现有测试不变
- [x] [core logic] `scripts.rs`：`scan` 和头部解析
      Purpose：列表的数据
      Verification：单元测试覆盖默认值、`#` / `//` / `--` 注释、`#!` 和声明之间有空行、遇到代码就停、icon 的几种写法、没有执行权限的文件和隐藏文件不列、符号链接能列
- [x] [core logic] `scripts.rs`：`start` / `Run`
      Purpose：运行、合并输出、结果、杀进程
      Verification：用临时脚本做单元测试：stdout/stderr 顺序、多字节字符和 `\t` 保留、退出码、`kill -9 $$` 得到 "Killed by signal 9"、没有执行权限、`sleep 100 &` 不会卡住且读线程会结束、没有换行的大块输出被截断、丢弃 `Run` 后子进程被杀
- [ ] [integration] `lib.rs`：Scripts 分组、图标、silent 运行 + HUD、托盘 "Open Scripts Folder…"、目录和 README
      Purpose：打通 silent 流程
      Verification：`cargo build`；你实际运行一个切换深色模式的脚本
- [ ] [integration] `script_view.rs` + `Mode::Output`
      Purpose：打通 output 流程
      Verification：运行 `seq 5000` 和一个每秒输出一行的脚本，看到输出陆续出现、界面不卡、状态变化；按 Escape 返回列表，搜索框有焦点，脚本被杀（`ps` 确认）
- [x] [docs] 更新 `AGENTS.md` 的架构部分（scripts、HUD、`.env`）

## Deviations from Design
- `hud::show` is `Hud::show`, a method of a `Copy` struct made by the root: it keeps the window, the timer and the root's owner, so it can be called from any owner.
- A script without `#!` does not fail: as from a terminal, `/bin/sh` runs it (the system's `execvp` fallback). The "no `#!`" failure is gone; the README says so.
- The output view's text is `selectable`, so the output can be copied with the mouse; no copy shortcut.
- `Run`'s `lines` / `done` are private behind `line()`, `line_now()` and `outcome(self)`: an `async move` block that used `run.lines` and `run.done` captured only those two fields (Rust 2021 captures disjoint fields), so the kill guard was dropped when `script_view` returned and every output script was killed at once ("Killed by signal 9"). With methods the block takes the whole `Run`.
- `@icon` also takes any other path (with a `/`), shown with the icon Finder gives it through the apps' icon cache (`icons::ensure`): apps, and System Settings panes' `/System/Library/ExtensionKit/Extensions/*.appex`, whose icons are the sidebar's (the old `.prefPane`s only get a generic document icon).
- A silent script that succeeds without output shows no HUD (was "✓ <name>"): one that opens a window, as most do, has nothing to say, and a script that wants a confirmation prints one.

## Status
Checked in the app: the scripts list and icons, a silent script's HUD (frosted glass, as the popup), Slow Output stopped with nothing left running when going back, `.env`'s PATH (Nix's folders first, `$HOME` and `$USER` expanded, taken without a restart). CI passes on macOS 15. Not yet checked: Count to 5000 after the `Run` fix, the HUD not taking the keyboard from the app in front, two HUDs in a row.
