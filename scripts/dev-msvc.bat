@echo off
rem 加载 MSVC 构建环境后启动 Tauri 开发模式（规避 Git link.exe 冲突）
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d E:\Codes\runphp
D:\Tools\mise\bin\mise.exe exec -- npm run tauri dev
