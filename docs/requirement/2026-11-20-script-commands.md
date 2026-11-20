# Script Commands

## Problem
有些事没有现成的 app 可开，比如切换深色模式、查服务状态，只能开终端敲命令。想把自己写的脚本放进 Tinycast 的启动器：搜到、回车就运行，不用开终端。思路来自 Raycast 的 Script Commands，但只为自己的脚本服务，格式不用兼容 Raycast。

## Scenarios
- 按一下就完事：在启动器里搜 "dark"，回车运行"切换深色模式"脚本。popup 关闭，屏幕上短暂弹出一个提示（HUD），显示脚本输出的最后一行；脚本失败（退出码非 0）时提示里说明失败和原因。
- 要看结果的查询：搜 "port"，回车运行"列出占用端口的进程"脚本。popup 不关，切到一个页面显示完整输出（可能有很多行），能返回启动器列表或按 Escape 关闭。
- 新增脚本：把脚本文件放进 `~/.config/tinycast/scripts/`，不用重启 Tinycast，它就出现在启动器列表里；删掉文件，它就消失。

## Scope
In:
- 固定目录 `~/.config/tinycast/scripts/` 里的脚本，列在启动器中，和 app、窗口命令一起搜索，也参与按最近和常用排序。
- 两种输出模式，每个脚本自己声明用哪种：
  - 关闭 popup 并用 HUD 显示最后一行（Raycast 的 silent）；
  - 在 popup 里显示完整输出（Raycast 的 fullOutput）。
- 脚本在文件里用 Tinycast 自己的简单格式声明以下信息，每项都可以不写：
  - name：列表里显示的名字，不写就用文件名（去掉扩展名）；
  - icon：一个 emoji，或者一个图片文件（PNG/SVG，相对路径相对脚本所在目录），不写就用默认的脚本图标；
  - 输出模式，不写就用 silent。
- 新做 HUD：Tinycast 目前没有。

Out（以后需要再加）:
- 兼容 Raycast 的 `@raycast.*` 元数据格式。
- compact 和 inline 模式：compact 和 silent 差别不大；inline 要在后台定时运行脚本，复杂很多。
- 脚本参数（运行前输入文字）。
- 可配置的脚本目录、多个目录。

## Constraints
- 从 Finder/菜单栏启动的 app 拿到的 PATH 很短（`/usr/bin:/bin:/usr/sbin:/sbin`），脚本里用 Homebrew、Nix 等装的命令会找不到。
  - 代码只在 PATH 前面加 `/usr/local/bin` 和 `~/.local/bin`。
  - Homebrew、Nix 这类不在代码里加：各人装法不同，谁排前面也因人而异。改由用户在 `~/.config/tinycast/.env`（和 `config.toml` 放在一起）里自己配置，例如 `PATH=/opt/homebrew/bin:$HOME/.nix-profile/bin:$PATH`。
  - `.env` 里的变量传给每个脚本；值里的 `$VAR` / `${VAR}` 会展开成已有的值（`$PATH` 指上一条说的 PATH），这样才能在前面或后面追加。
  - 改了 `.env` 不用重启 Tinycast，下次运行脚本就生效；文件有错误的行在托盘菜单里提示，和 `config.toml` 的问题一样。
- 脚本可能运行很久或卡住：不能卡住 UI，silent 模式下 popup 关闭后脚本仍要跑完并弹出结果。
- 不能用原生弹窗（会抢焦点让 popup 关闭），提示和错误都在 popup 或 HUD 里显示。

## Success Criteria
- 放进脚本目录的可执行脚本几秒内出现在启动器里，删掉后消失，不用重启。
- 脚本里写了 name 和 icon 的，列表里显示那个名字和图标（emoji 或图片），搜索按 name 匹配；没写的显示文件名和默认图标。
- silent 脚本：回车后 popup 关闭，脚本结束时 HUD 显示最后一行输出，几秒后自动消失；失败时 HUD 显示失败。
- 完整输出的脚本：回车后 popup 里显示脚本的 stdout 和 stderr，运行期间有"运行中"的状态，能返回列表。
- 脚本里调用 `/usr/local/bin`、`~/.local/bin` 里的命令能直接运行。
- 在 `.env` 里写上 `PATH=/opt/homebrew/bin:$PATH` 后，脚本里调用 Homebrew 装的命令（如 `jq`）能正常运行，不用在脚本里自己设 PATH，也不用重启 Tinycast。
- 运行中的脚本不会让启动器卡顿。
