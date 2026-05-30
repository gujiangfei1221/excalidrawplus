# Excalidraw Desktop — 带云同步的桌面白板应用

基于 [Excalidraw](https://github.com/excalidraw/excalidraw) 二次开发的桌面端应用，使用 Tauri v2 打包，集成腾讯云 COS 实现跨设备文件同步。

## 功能特性

- **本地优先**：所有操作先写入本地，断网也能正常使用
- **云端同步**：通过腾讯云 COS 自动同步文件，支持多设备协同
- **文件管理**：侧边栏文件列表，支持新建、重命名、导入、删除
- **冲突处理**：多设备同时编辑时自动检测冲突，生成冲突副本
- **分享功能**：一键导出画布为 PNG 并上传 COS，生成公开链接
- **跨平台**：支持 macOS（Intel / Apple Silicon）和 Windows

## 架构

```
┌─────────────────────────────────────────────────┐
│              Frontend (React + TypeScript)        │
│  Excalidraw Editor │ FileListSidebar │ Toolbar   │
└────────────────────────┬────────────────────────┘
                         │ Tauri IPC
┌────────────────────────┴────────────────────────┐
│              Backend (Rust / Tauri v2)            │
│  SyncEngine │ CosClient │ Database │ FileStore   │
└────────────────────────┬────────────────────────┘
                         │
          ┌──────────────┴──────────────┐
          │                             │
   ┌──────┴──────┐             ┌───────┴───────┐
   │ 本地存储     │             │  腾讯云 COS    │
   │ SQLite      │             │ .excalidraw   │
   │ files/      │             │ manifest.json │
   └─────────────┘             └───────────────┘
```

| 组件 | 职责 |
|------|------|
| SyncEngine | 协调同步：manifest 轮询、上传队列、冲突检测 |
| CosClient | S3 兼容的 COS 客户端（基于 aws-sdk-s3） |
| Database | SQLite 存储文件元数据、同步队列、COS 配置 |
| FileStore | 本地文件系统读写 `.excalidraw` JSON |
| manifest.json | COS 上的文件索引，跨设备对账清单 |

## 同步机制

采用类 Git 的操作模型：

- **Push 推送**：将本地修改上传到 COS
- **Pull 拉取**：从 COS 拉取最新版本覆盖本地
- **自动同步**：后台每 30 秒轮询 manifest，5 秒处理上传队列
- **冲突检测**：基于三方 hash 对比（本地 / 远端 / 基线），冲突时保留双方副本

## 开发

### 环境要求

- Node.js 20+
- Rust 1.77+
- Yarn (通过 Corepack)
- Tauri CLI v2 (`cargo install tauri-cli --version "^2"`)

### 本地开发

```bash
# 安装前端依赖
yarn install

# 启动开发模式（前端 + Tauri 窗口热重载）
cd src-tauri
cargo tauri dev
```

### 构建桌面包

```bash
# macOS
./build-desktop.command --no-proxy

# Windows
build-desktop.bat -NoProxy
```

### 常用命令

```bash
yarn test:typecheck    # TypeScript 类型检查
yarn test:update       # 运行测试（含快照更新）
yarn fix               # 自动修复格式和 lint
cd src-tauri && cargo test  # Rust 后端测试
```

## CI/CD

推送到 `main` 分支自动触发 GitHub Actions：

1. 并行构建 macOS arm64 / macOS x64 / Windows x64
2. 自动创建 GitHub Release，附带安装包下载

在仓库的 [Releases](../../releases) 页面下载最新版本。

## 项目结构

```
├── excalidraw-app/src/cloud-sync/   # 云同步前端（React 组件）
├── src-tauri/src/                   # Rust 后端
│   ├── sync_engine.rs               # 同步引擎核心
│   ├── cos_client.rs                # COS 客户端
│   ├── database.rs                  # SQLite 数据层
│   ├── file_store.rs                # 本地文件存储
│   ├── commands.rs                  # Tauri 命令层
│   └── connectivity.rs             # 网络连接监测
├── packages/excalidraw/             # Excalidraw 核心库
├── scripts/                         # 构建脚本
└── .github/workflows/               # CI/CD 流水线
```

## 致谢

本项目基于 [Excalidraw](https://github.com/excalidraw/excalidraw) 开源项目开发。

## 许可证

本项目采用 MIT 许可证，详见 [LICENSE](./LICENSE)。

Excalidraw 原始版权声明：

> MIT License
>
> Copyright (c) 2020 Excalidraw
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
