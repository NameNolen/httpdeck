# HttpDeck

一个用 **Rust + Tauri 2** 编写的桌面 HTTP 客户端，目标是在不启动 IntelliJ IDEA 的情况下，直接打开并运行你现有的 `.http` 文件。

界面采用深色开发者风格：左侧是工作区目录树与置顶文件，中间是 `.http` 编辑器与请求列表，底部是响应面板。

---

## 功能列表

### 1. HTTP 客户端

- 支持 `GET` / `POST` / `PUT` / `DELETE` / `PATCH` / `HEAD` / `OPTIONS` / `TRACE` / `CONNECT`
- 完整请求头、请求体编辑，URL 中使用变量
- 请求体从文件读取：`< ./payload.json`
- multipart 请求体中的文件引用：行内 `< ./file.bin` 会自动读取文件内容
- 自动 Cookie 会话（同一工作区内跨请求保持）
- 自动跟随重定向（最多 10 次），可用 `// @no-redirect` 关闭
- 请求超时设置：`// @timeout 5000`（毫秒，默认 60 秒）
- 可关闭 Cookie：`// @no-cookie-jar`
- 可忽略 TLS 证书校验：`// @no-ssl-verification`
- 支持 gzip / brotli / deflate 响应自动解压
- 二进制响应自动识别并以 Base64 展示
- 保存响应到文件：`>> ./response.json`（追加）、`>>! ./response.json`（覆盖）、`<> ./response.json`（覆盖）

### 2. IntelliJ `.http` 语法兼容

- `###` 分隔请求，`### 名称` 作为请求标题
- 文件变量：`@host = https://example.com`（支持引号）
- 变量替换：`{{host}}`，支持嵌套变量
- 动态变量：`{{$uuid}}`、`{{$timestamp}}`、`{{$isoTimestamp}}`、`{{$randomInt}}`、`{{$random.uuid}}`、`{{$random.alphabetic(8)}}`、`{{$random.alphanumeric(12)}}`、`{{$random.hexadecimal(16)}}`、`{{$random.email}}`、`{{$processEnv(NAME)}}`
- 请求级指令：`// @name`、`// @no-redirect`、`// @no-cookie-jar`、`// @no-log`、`// @timeout`、`// @no-ssl-verification`
- 多行 URL（缩进续行）
- 前置脚本：请求前 `> {% ... %}` 或 `> ./scripts/pre.js`
- 后置脚本：请求后 `> {% ... %}` 或 `> ./scripts/response.js`
- 兼容 `#` 与 `//` 注释

### 3. JavaScript 脚本（QuickJS 运行时）

- `client.global.set(name, value)` / `get` / `unset` / `clear` / `isEmpty`
- `client.test(name, function () { ... })`：收集测试结果
- `client.assert(condition, message)`：断言失败会记入测试结果
- `client.log(...)` 与 `console.log(...)`：输出到响应面板“控制台”
- `request.variables.set(name, value)`：覆盖本次请求的变量
- `request.method` / `request.url` / `request.body`
- `response.status`、`response.body`（JSON 自动解析为对象）、`response.bodyText`、`response.headers.valueOf(name)`、`response.headers.valuesOf(name)`、`response.contentType.mimeType` / `charset`
- 脚本文件路径相对于当前 `.http` 文件解析

### 4. 环境与变量

- 完整读取 `http-client.env.json`
- 完整读取 `http-client.private.env.json`（覆盖公有环境）
- 支持 `$shared` 共享变量
- 顶部环境选择器，选择结果持久化到 `.http-client/workspace.json`
- 变量优先级：请求级变量 > `client.global` > 环境变量 > 文件变量 > 动态变量

### 5. 工作区

- 通过系统目录选择器“添加目录”，多个目录并列显示在左侧，互不切换、互不影响
- 历史目录自动记录并持久化到 `%APPDATA%\HttpDeck\recent.json`，下次启动自动恢复（最多 8 个）
- 单击工作区标题切换当前目录；右键工作区标题可“移除目录”（不删除磁盘文件）
- 左侧目录树扫描全部 `.http` / `.rest` 文件，自动裁剪空目录
- 工作区标题的 ▾ / ▸ 可折叠整个目录；目录行的 ▸ 可折叠子目录
- 文件行右键（或悬停出现的 ▸）可展开文件内的 HTTP 请求列表，显示方法、名称与行号，点击直接跳到编辑器对应请求，类似 IntelliJ 的文件结构视图
- 文件置顶（星标）显示在侧栏“置顶常用”，配置持久化在工作区 `.http-client/workspace.json`，可随仓库提交
- 多标签打开文件，未保存时显示蓝点
- 请求行前的 ▶ 按钮（编辑器左侧行号栏）可直接运行单个请求
- 按文件运行全部请求
- 按目录批量运行（递归执行目录下所有 `.http` / `.rest` 文件）
- 响应面板：响应体 / 响应头 / 测试 / 控制台 / 实际请求五个视图
- JSON 响应自动格式化，可切换原始文本
- `client.global` 变量查看与清空
- 快捷键：`Ctrl+S` 保存、`Ctrl+Enter` 运行光标所在请求

---

## 待办 / 路线图

### 高优先级

- [x] 请求历史记录（持久化、搜索、重发）
- [x] 响应面板高度可拖拽
- [x] JSON 响应折叠 / 搜索 / 复制为 cURL
- [x] 请求取消（长时间运行或大响应时）
- [x] 目录级 `http-client.env.json`（目前只读取工作区根目录）
- [x] `.env` 文件支持
- [x] Cookie 持久化与 Cookie 查看器
- [x] 代理设置（HTTP / SOCKS5）
- [x] 脚本执行超时与更强的沙箱限制（防止死循环）

### 中优先级

- [ ] 使用 `reqwest::multipart` 结构化构建 multipart（当前为原始 body 替换文件引用）
- [ ] 完整 charset 解码（目前按 UTF-8 解码，非法时回退 Base64）
- [ ] 变量自动补全与跳转定义
- [ ] 响应内容大小阈值与超大响应虚拟滚动
- [ ] 请求级超时/重定向状态的 UI 提示
- [ ] 目录右键菜单（运行、置顶、在资源管理器中打开、从列表移除）
- [ ] 新建 / 重命名 / 删除 `.http` 文件
- [ ] 最近打开文件列表

### 低优先级 / 暂不支持

- [ ] WebSocket
- [ ] Server-Sent Events
- [ ] gRPC
- [ ] GraphQL 专用请求体
- [ ] OAuth 2.0 / Digest / NTLM 认证助手
- [ ] 客户端证书（mTLS）配置界面
- [ ] TypeScript 脚本（IntelliJ 本身也不支持，需额外内置转译器）
- [ ] macOS / Linux 打包验证（Tauri 支持，但当前只验证 Windows）
- [ ] 自动化端到端测试

> 兼容性目标：`.http` 语法与常用脚本 API 对齐 IntelliJ IDEA HTTP Client。未列出的冷门 API、IntelliJ 私有扩展与 IDE 集成能力不在当前范围内。

---

## 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面框架 | Tauri 2 |
| 后端 | Rust（tokio、reqwest、rquickjs / QuickJS、serde） |
| 前端 | React 18 + TypeScript + Vite |
| 编辑器 | CodeMirror 6（自定义 `.http` 语法高亮） |
| HTTP | reqwest（rustls、cookie_store、gzip / brotli / deflate） |
| 脚本 | rquickjs 内嵌 QuickJS，脚本运行在独立上下文中 |

## 本地开发

环境要求：

- Node.js 18+
- Rust stable（MSVC toolchain）
- Windows 10/11 + WebView2 Runtime（Windows 11 已内置）
- Visual Studio Build Tools（C++ 桌面开发）

```powershell
npm install
npm run tauri dev
```

## 构建 Windows 安装包

```powershell
npm run tauri build
```

产物：

- 可执行文件：`src-tauri/target/release/httpdeck.exe`（可直接运行，无需安装）
- 安装包：`src-tauri/target/release/bundle/nsis/HttpDeck_0.1.0_x64-setup.exe`

> 打包安装包时，Tauri 会自动从 GitHub 下载 NSIS 工具链到 `%LOCALAPPDATA%\tauri\NSIS`。
> 如果网络无法访问 GitHub，打包会在 `Downloading ... nsis-3.13.zip` 处失败；
> 此时可以直接使用上面的 `httpdeck.exe`（便携版），或预先安装 NSIS 3.13 并复制到该缓存目录后重试。

## 使用方式

1. 点击“添加目录”，选择一个包含 `.http` 文件的目录；已添加的目录会并列显示在左侧并记入历史。
2. 在左侧目录树中打开文件，或把常用文件点亮星标置顶。
3. 在顶部选择环境（读取工作区根目录的 `http-client.env.json`）。
4. 点击请求条目的 ▶ 运行单个请求，或点击“运行全部”；也可以点编辑器行号栏的 ▶。
5. 在底部响应面板查看响应、响应头、测试断言、脚本日志与实际请求。
6. 目录行的 ▶ 可以递归运行整个目录。
7. 右键左侧的工作区标题可“移除目录”（只从列表移除，不删除磁盘文件）。

也可以从命令行直接添加工作区：

```powershell
.\httpdeck.exe D:\projects\my-api
```

示例工作区在仓库的 `httpdeck/examples` 目录（默认指向 `https://httpbin.org`，需要联网）：

```text
C:\Users\renpe\Documents\Trae-CN\default\httpdeck\examples
```

用“添加目录”选中它即可，也可以直接运行 `httpdeck.exe examples`。

示例 `.http`：

```http
@host = https://httpbin.org

### 获取请求信息
GET {{host}}/get?from=httpdeck
Accept: application/json

> {%
    client.test("状态码为 200", function () {
        client.assert(response.status === 200, "期望 200，实际 " + response.status);
    });
    client.log("origin:", response.body.origin);
    client.global.set("lastOrigin", response.body.origin);
%}
```

## 项目结构

```
httpdeck/
├─ src/                     # React 前端
│  ├─ components/           # 侧栏、编辑器、响应面板
│  ├─ lib/httpLanguage.ts   # CodeMirror .http 语法
│  ├─ api.ts                # Tauri IPC 类型与封装
│  └─ App.tsx
├─ src-tauri/
│  └─ src/
│     ├─ parser.rs          # .http 解析器
│     ├─ variables.rs       # 变量与动态变量替换
│     ├─ envfile.rs         # 环境文件合并
│     ├─ script.rs          # QuickJS 脚本运行时
│     ├─ executor.rs        # HTTP 执行器
│     ├─ workspace.rs       # 目录扫描与工作区配置
│     ├─ recent.rs          # 历史目录记录（%APPDATA%\HttpDeck\recent.json）
│     ├─ models.rs
│     └─ lib.rs             # Tauri 命令注册
└─ examples/                # 可打开的示例工作区
```

## 已知限制

- 脚本只提供 `client` / `request` / `response` / `console`，没有文件系统与网络访问能力；单段脚本超过 5 秒会被中断。
- 响应文本当前仅按 UTF-8 解码，非法字节会以 Base64 返回。
- 代理、认证助手尚未实现（见待办）。
- 仅验证 Windows 构建。
