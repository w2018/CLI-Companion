# 端到端验证：daemon 启动 → 命名管道 RPC ping → daemon 优雅关闭
# 用法：powershell -File scripts\e2e_verify.ps1

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot | Split-Path
$daemon = Join-Path $root "target\release\cli-companion-daemon.exe"
$dataDir = Join-Path $root ".dev-data"
$pipeName = "cli-companion-daemon"

Write-Host "[1] 启动 daemon（数据目录: $dataDir）"
Start-Process -FilePath $daemon -ArgumentList "--data-dir", $dataDir -WindowStyle Hidden
Start-Sleep -Seconds 2

try {
    Write-Host "[2] 连接命名管道..."
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", $pipeName, [System.IO.Pipes.PipeDirection]::InOut)
    $pipe.Connect(5000)
    Write-Host "    管道连接成功"

    Write-Host "[3] 发送 system.ping 请求（4字节长度前缀 + JSON）"
    $json = '{"jsonrpc":"2.0","id":1,"method":"system.ping"}'
    $payload = [System.Text.Encoding]::UTF8.GetBytes($json)
    $writer = New-Object System.IO.BinaryWriter($pipe)
    $writer.Write([BitConverter]::GetBytes([UInt32]$payload.Length))
    $writer.Write($payload)
    $writer.Flush()

    Write-Host "[4] 读取响应帧"
    $reader = New-Object System.IO.BinaryReader($pipe)
    $lenBytes = $reader.ReadBytes(4)
    $len = [BitConverter]::ToUInt32($lenBytes, 0)
    $respBytes = $reader.ReadBytes($len)
    $resp = [System.Text.Encoding]::UTF8.GetString($respBytes)
    Write-Host "    响应: $resp"

    if ($resp -match '"ok":true' -and $resp -match '"daemon_version"') {
        Write-Host "[PASS] system.ping 往返成功，daemon 工作正常" -ForegroundColor Green

        Write-Host "[5] 发送 system.info"
        $json2 = '{"jsonrpc":"2.0","id":2,"method":"system.info"}'
        $p2 = [System.Text.Encoding]::UTF8.GetBytes($json2)
        $writer.Write([BitConverter]::GetBytes([UInt32]$p2.Length))
        $writer.Write($p2)
        $writer.Flush()
        $len2 = [BitConverter]::ToUInt32($reader.ReadBytes(4), 0)
        $resp2 = [System.Text.Encoding]::UTF8.GetString($reader.ReadBytes($len2))
        Write-Host "    响应: $resp2"

        if ($resp2 -match '"running_as_service":false') {
            Write-Host "[PASS] system.info 正常" -ForegroundColor Green
        }

        Write-Host "[6] 发送 daemon.shutdown（停止全部服务）"
        $json3 = '{"jsonrpc":"2.0","id":3,"method":"daemon.shutdown","params":{"stop_services":true}}'
        $p3 = [System.Text.Encoding]::UTF8.GetBytes($json3)
        $writer.Write([BitConverter]::GetBytes([UInt32]$p3.Length))
        $writer.Write($p3)
        $writer.Flush()
        $len3 = [BitConverter]::ToUInt32($reader.ReadBytes(4), 0)
        $resp3 = [System.Text.Encoding]::UTF8.GetString($reader.ReadBytes($len3))
        Write-Host "    响应: $resp3"
    } else {
        Write-Host "[FAIL] ping 响应异常" -ForegroundColor Red
        exit 1
    }
    $pipe.Dispose()
} catch {
    Write-Host "[FAIL] 验证失败: $_" -ForegroundColor Red
    # 兜底清理
    Get-Process cli-companion-daemon -ErrorAction SilentlyContinue | Stop-Process -Force
    exit 1
}

Write-Host "[7] 等待 daemon 退出..."
Start-Sleep -Seconds 3
$alive = Get-Process cli-companion-daemon -ErrorAction SilentlyContinue
if ($alive) {
    Write-Host "[WARN] daemon 未自行退出，强制结束"
    $alive | Stop-Process -Force
} else {
    Write-Host "[PASS] daemon 已优雅退出" -ForegroundColor Green
}

Write-Host "[8] 查看 daemon 日志尾部："
Get-Content (Join-Path $dataDir "logs\daemon.log") -Tail 10 -ErrorAction SilentlyContinue
Write-Host "`n=== 端到端验证完成 ===" -ForegroundColor Green
