# Windows 构建脚本
# 使用 schannel（系统 TLS），全程零 C 依赖，无需安装任何编译器/工具链。
# 产物为单个 exe（Rust std 静态链接），可直接拷走运行。
param()

cargo build --release --no-default-features --features tls-native
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "==> 产物: target\release\educe.exe"
Write-Host "    运行: .\target\release\educe.exe serve"
