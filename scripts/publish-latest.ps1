# 拉取 GitHub Actions 最新一次成功构建的 exe，并上传到飞书云盘
# 用法：powershell -File scripts/publish-latest.ps1 [-Branch explore/native-ui] [-RunId 12345]
param(
    [string]$Branch = "",
    [string]$RunId = "",
    [string]$Repo = "NameNolen/httpdeck"
)

$ErrorActionPreference = "Stop"

if (-not $Branch) {
    $Branch = (git rev-parse --abbrev-ref HEAD).Trim()
}

if (-not $RunId) {
    $RunId = (gh run list -R $Repo -b $Branch -L 1 --status success --json databaseId --jq '.[0].databaseId')
    if (-not $RunId) {
        throw "分支 $Branch 上没有成功的构建，请先推送触发 Actions"
    }
}
Write-Host "使用运行记录：$RunId（分支 $Branch）"

$artifact = (gh api "repos/$Repo/actions/runs/$RunId/artifacts" --jq '.artifacts[0].name')
if (-not $artifact) {
    throw "运行 $RunId 没有产物"
}
Write-Host "下载产物：$artifact"

$work = Join-Path $env:TEMP ("httpdeck-artifact-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $work | Out-Null
gh run download $RunId -R $Repo -n $artifact -D $work
if ($LASTEXITCODE -ne 0) {
    throw "下载产物失败"
}

Push-Location $work
try {
    foreach ($file in Get-ChildItem -File) {
        Write-Host "上传：$($file.Name)（$([math]::Round($file.Length / 1MB, 1)) MB）"
        lark-cli drive +upload --file $file.Name --as bot
        if ($LASTEXITCODE -ne 0) {
            throw "上传 $($file.Name) 失败"
        }
    }
} finally {
    Pop-Location
    Remove-Item -Recurse -Force $work
}
