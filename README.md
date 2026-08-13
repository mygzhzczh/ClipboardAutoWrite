# ClipboardAutoWrite

Windows 剪贴板自动保存工具。常驻后台监听剪贴板变化，自动将复制的文本按日期保存到文本文件。

## 功能

- 监听剪贴板变化，自动保存复制的内容
- 按日期生成文件（如 `Clipboard20260813.txt`），保存在 exe 同级目录
- 新内容写入文件顶部，重复内容自动跳过
- 支持开机自启动（写入注册表，无需管理员权限）
- 休眠唤醒后自动恢复监听

## 使用方法

1. 下载 `clipboard_autowrite.exe`
2. 双击运行即可，无需安装
3. 复制任意文本，内容会自动保存到 exe 同级目录下的 `Clipboard<日期>.txt`

## 构建

```bash
cargo build --release
```

产物位于 `target/release/clipboard_autowrite.exe`。

## 说明

- 文件内容格式：每次复制会记录时间戳，内容之间用分隔线隔开
- 以管理员身份运行可额外创建计划任务，实现异常退出后自动重启