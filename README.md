# FileFloat Tauri

桌面快捷文件搜索工具 (Desktop Quick File Search Tool)。基于 Tauri + React 构建，底层集成了 [Everything](https://www.voidtools.com/) 搜索引擎，提供极速的本地文件搜索体验。

## ✨ 特性 (Features)

- **极速搜索**：内置 Everything 引擎，毫秒级响应本地文件搜索。
- **全局快捷键**：按下 `Alt + Space` 瞬间唤出搜索框，再次按下即可收起。
- **悬浮球模式**：平时以小巧的悬浮球形式存在，支持鼠标拖拽调整位置。
- **边缘吸附**：悬浮球靠近屏幕边缘时会自动吸附并隐藏，悬停即可呼出。
- **智能交互**：
  - 支持键盘上下键选择结果，`Enter` 键由系统默认应用打开文件。
  - 右键菜单支持：打开文件、打开所在目录、复制、剪切、删除、复制完整路径。
  - 搜索框展开后闲置 1 分钟自动收起，点击放大镜内部红点也可快速收起。
- **无感体验**：无黑窗口闪烁，沉浸式纯净的桌面体验。

## 🚀 安装使用 (Installation)

你可以直接在 [Releases](https://github.com/qiebo/filefloat-tauri/releases) 页面下载编译好的单文件版 `.exe` 或 `.msi`（NSIS setup）安装包，双击即可运行。

### 本地编译 (Build from source)

**环境要求**：
- [Node.js](https://nodejs.org/) (推荐 v18+)
- [Rust](https://www.rust-lang.org/)
- Windows 开发依赖 (C++ build tools)

**克隆项目**：
```bash
git clone https://github.com/qiebo/filefloat-tauri.git
cd filefloat-tauri
```

**安装依赖**：
```bash
npm install
```

**开发环境运行**：
```bash
npm run tauri dev
```

**打包发布版 (Release)**：
```bash
npm run tauri build
```
编译产物位于 `src-tauri/target/release/` 目录下（包含单文件 `filefloat-tauri.exe` 以及 `bundle/nsis/` 目录下的安装包）。

## 🛠️ 技术栈 (Tech Stack)

- **前端**：React 18, Vite, Lucide React (图标库)
- **后端**：Rust, Tauri v2
- **搜索**：集成 Voidtools Everything HTTP API (同时支持 Windows Search / 跨盘文件遍历作为 Fallback)

## 📄 License
MIT
