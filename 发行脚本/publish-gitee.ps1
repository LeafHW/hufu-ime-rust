# publish-gitee.ps1 - 把发版产物同步到 Gitee（镜像仓 Release 附件 + 更新清单）
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File E:\DSH-KF\hufu\发行脚本\publish-gitee.ps1 -Version 1.9.9
#   ... -Version 1.9.9 -DryRun          # 只打印将要做的动作，不写任何东西
#   ... -Version 1.9.9 -SkipPush        # 只传附件与清单（代码镜像由别的流程推）
#
# 设计约束（2026-11-03 实测得出，别改回去）：
#   * 附件单文件硬上限 100 MB，脚本在 90 MB 处主动跳过；
#   * 每仓库附件总量 1 GB（25 MB/版 ≈ 38 版），默认只保留最近 3 版的 exe；
#   * Gitee 下载不支持 Range，清单里必须带 sha256 + size，客户端只能整包重下；
#   * 更新清单不能走 Gitee API（未认证 60 次/小时），必须走 raw 静态文件；
#   * 清单写入失败/附件失败都不阻断发版，只告警（铁律：不能因为镜像挂掉卡住主流程）。
#
# 退出码：0 = 主流程完成（可能有告警）；1 = 参数/前置条件错误。

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    [string]$ExePath = '',

    [switch]$SkipPush,

    [switch]$PushCode,

    [switch]$PublishSite,

    [switch]$DryRun,

    [int]$KeepReleases = 3
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$TokenFile   = 'E:\DSH-KF\gitee-token.txt'
$Owner       = 'leafHW'
$Repo        = 'hufu-ime-rust'
$UpdatesRepo = 'hufu-updates'
$SiteRepoDir = 'E:\DSH-KF\hufu-site'
$RepoDir     = 'E:\DSH-KF\hufu'
$MaxUploadMB = 90
$Api         = "https://gitee.com/api/v5/repos/$Owner"

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Warn2($msg) { Write-Host "  [WARN] $msg" -ForegroundColor Yellow }

# PS5.1 坑：脚本 $ErrorActionPreference='Stop' 时，把原生命令的 stderr 用 2>&1
# 捕获会抛 NativeCommandError（git 的 "remote: ..." 横幅就是 stderr）。
# 统一走这个 helper：调用期间临时切 Continue，返回输出与退出码。
function Invoke-Native {
    param([string]$Exe, [string[]]$ArgList)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Exe @ArgList 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
    return [pscustomobject]@{ Output = @($out); Code = $code }
}

$Version = $Version.Trim().TrimStart('v', 'V')
$Tag = "v$Version"

if (-not (Test-Path $TokenFile)) {
    Write-Host "找不到令牌文件：$TokenFile" -ForegroundColor Red
    exit 1
}
$Token = (Get-Content $TokenFile -Raw).Trim()
if ($Token.Length -lt 16) {
    Write-Host "令牌文件内容无效（长度 $($Token.Length)）" -ForegroundColor Red
    exit 1
}

if (-not $ExePath) {
    $ExePath = "E:\DSH-KF\hufu-发行\HuFuSetup-$Version.exe"
}
if (-not (Test-Path $ExePath)) {
    Write-Host "找不到安装包：$ExePath" -ForegroundColor Red
    exit 1
}
$ExeItem  = Get-Item $ExePath
$ExeName  = $ExeItem.Name
$Sha256   = (Get-FileHash $ExePath -Algorithm SHA256).Hash.ToLower()
$Size     = $ExeItem.Length
$SizeMB   = [math]::Round($Size / 1MB, 1)

Write-Step "版本 $Tag · $ExeName · $SizeMB MB · sha256 $($Sha256.Substring(0,16))…"

# ---------------------------------------------------------------------------
# 1. 推代码与 tag 到 Gitee 镜像仓（失败不阻断）
# ---------------------------------------------------------------------------
if (-not $SkipPush) {
    Write-Step "推送 tags 到 Gitee"
    if ($DryRun) {
        Write-Host "  (DryRun) 跳过实际 push"
    } else {
        $env:GIT_TERMINAL_PROMPT = '0'
        $remote = "https://${Owner}:$Token@gitee.com/$Owner/$Repo.git"
        # 默认只推 tags：Release / 附件 / 清单都不依赖分支。
        # Gitee master 与工作分支历史上已分叉（2026-11-04 实测：Gitee master 带 3 个
        # GitHub-master 的文档提交，不在工作分支上），整仓镜像必须显式 -PushCode，
        # 且只能 force 覆盖，否则永远 non-fast-forward。
        $pushArgs = @('-C', $RepoDir, '-c', 'credential.helper=', '-c', 'http.extraheader=', 'push', $remote, '--tags', "+refs/tags/$Tag")
        if ($PushCode) {
            Write-Warn2 "PushCode：强制把当前分支覆盖到 Gitee master"
            $pushArgs += '+HEAD:refs/heads/master'
        }
        $r = Invoke-Native 'git' $pushArgs
        if ($r.Code -eq 0) {
            Write-Ok "push 完成"
        } else {
            Write-Warn2 "push 失败（不阻断发版）：$($r.Output | Select-Object -Last 3 | Out-String)"
        }
    }
}

# ---------------------------------------------------------------------------
# 2. 确保 Release 存在
# ---------------------------------------------------------------------------
Write-Step "确保 Gitee Release $Tag 存在"
$Release = $null
try {
    $Release = Invoke-RestMethod -Uri "$Api/$Repo/releases/tags/$Tag`?access_token=$Token" -TimeoutSec 30
} catch {
    $Release = $null
}
# Gitee 对「tag 存在但 release 不存在」会返回 HTTP 200 + 正文 null，
# PowerShell 可能拿到空对象 → 必须校验 id，否则后面 attach_files 拼出
# /releases//attach_files 直接 405（2026-11-04 v1.9.9.1 实测）。
if ($Release -and -not $Release.id) { $Release = $null }

if (-not $Release) {
    if ($DryRun) {
        Write-Host "  (DryRun) 将创建 Release $Tag"
        return
    }
    $body = @{
        access_token     = $Token
        tag_name         = $Tag
        name             = "HuFu 虎符输入法 $Tag"
        body             = "国内加速分发（镜像自 GitHub Releases）。安装包见下方附件。`n`n整句模型（约 525 MB，可选）在 ModelScope 单独下载（国内直连、免登录）：[点此下载 模型.rar](https://www.modelscope.cn/models/leafyin/hufu-ime/resolve/master/%E6%A8%A1%E5%9E%8B.rar?view=false)`n`n模型仓库页：[ModelScope · hufu-ime](https://www.modelscope.cn/models/leafyin/hufu-ime)"
        target_commitish = 'master'
    }
    $Release = Invoke-RestMethod -Method Post -Uri "$Api/$Repo/releases" -Body $body -TimeoutSec 60
    Write-Ok "已创建 Release id=$($Release.id)"
} else {
    Write-Ok "已存在 Release id=$($Release.id)"
}

$ExistingNames = @()
if ($Release.assets) { $ExistingNames = @($Release.assets | ForEach-Object { $_.name }) }

# ---------------------------------------------------------------------------
# 3. 上传安装包附件（同名跳过；>90 MB 跳过；用 curl.exe 走 multipart）
# ---------------------------------------------------------------------------
if ($ExistingNames -contains $ExeName) {
    Write-Ok "附件已存在，跳过上传：$ExeName"
} elseif ($SizeMB -gt $MaxUploadMB) {
    Write-Warn2 "安装包 $SizeMB MB 超过 $MaxUploadMB MB 安全线，跳过 Gitee 上传（客户端仍可从 GitHub 下载）"
} elseif ($DryRun) {
    Write-Host "  (DryRun) 将上传 $ExeName 到 Release $($Release.id)"
} else {
    Write-Step "上传附件 $ExeName"
    $attachUrl = "$Api/$Repo/releases/$($Release.id)/attach_files"
    $r = Invoke-Native 'curl.exe' @('-sS', '--max-time', '600', '-X', 'POST', $attachUrl, '-F', "access_token=$Token", '-F', "file=@$ExePath")
    $resp = ($r.Output -join "`n")
    if ($r.Code -eq 0 -and $resp -match '"browser_download_url"') {
        Write-Ok "附件上传成功"
    } else {
        Write-Warn2 "附件上传失败（不阻断发版）：$resp"
    }
}

# ---------------------------------------------------------------------------
# 4. 生成并推送更新清单（hufu-updates 小仓 raw + 站点同源副本）
# ---------------------------------------------------------------------------
# 清单的 date 用 Release 的发行日期：同版本重复跑必须幂等，不能用「今天」
$Date = if ($Release.created_at) {
    ([datetime]$Release.created_at).ToString('yyyy-MM-dd')
} else {
    (Get-Date).ToString('yyyy-MM-dd')
}
$Manifest = [ordered]@{
    latest   = $Version
    min      = '1.9.0'
    date     = $Date
    name     = "HuFu 虎符输入法 $Tag"
    page_url = 'https://www.hufuime.top/releases/'
    sha256   = $Sha256
    size     = $Size
    sources  = @(
        [ordered]@{ name = 'Gitee';  url = "https://gitee.com/$Owner/$Repo/releases/download/$Tag/$ExeName" }
        [ordered]@{ name = 'GitHub'; url = "https://github.com/LeafHW/$Repo/releases/download/$Tag/$ExeName" }
    )
}
$Json = ($Manifest | ConvertTo-Json -Depth 6)

# 本地副本：站点同源清单（浏览器 main.js 读它，避免 GitHub API 限流）
$SiteManifestPath = Join-Path $SiteRepoDir 'update.json'
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

if ($DryRun) {
    Write-Host "  (DryRun) 清单内容：" -ForegroundColor DarkGray
    Write-Host $Json -ForegroundColor DarkGray
} else {
    Write-Step "写入站点清单 $SiteManifestPath"
    [IO.File]::WriteAllText($SiteManifestPath, $Json, $Utf8NoBom)
    Write-Ok "已写入（记得随站点一起提交发布）"

    # 更新日志页快照：页面已改为「本地快照优先」，这里每次发版刷新，
    # 保证国内（GitHub API 不可达）也能看到最新版本与历史。
    # 抓取失败就保留旧快照——页面本来就能正常渲染。
    Write-Step "刷新站点更新日志快照（releases/cache.json）"
    $CachePath = Join-Path $SiteRepoDir 'releases\cache.json'
    $tmpRel = Join-Path $env:TEMP 'hufu-gh-releases.json'
    $ghFile2 = 'E:\DSH-KF\gh-token.txt'
    $gh2 = if (Test-Path $ghFile2) { (Get-Content $ghFile2 -Raw).Trim() } else { '' }
    $curlArgs2 = @('-sS', '--max-time', '60', '-H', 'Accept: application/vnd.github+json',
                   '-o', $tmpRel,
                   'https://api.github.com/repos/LeafHW/hufu-ime-rust/releases?per_page=100')
    if ($gh2) { $curlArgs2 = @('-sS', '--max-time', '60', '-H', 'Accept: application/vnd.github+json', '-H', "Authorization: Bearer $gh2", '-o', $tmpRel, 'https://api.github.com/repos/LeafHW/hufu-ime-rust/releases?per_page=100') }
    $r2 = Invoke-Native 'curl.exe' $curlArgs2
    try {
        # PS5.1 Get-Content 默认按 ANSI 解码，UTF-8 的 JSON 会乱码 → 解析失败
        $raw = Get-Content $tmpRel -Raw -Encoding UTF8
        if ($raw -and $raw.TrimStart().StartsWith('[')) {
            # 只保留页面用得到的字段与最近 12 版：GitHub 完整 payload 近 400 KB，
            # 国内首屏加载会明显变慢（2026-11-04 实测 390 KB → 52 KB）。
            $rel = $raw | ConvertFrom-Json
            $rows = @()
            foreach ($r in ($rel | Select-Object -First 12)) {
                $assets = @()
                foreach ($a in @($r.assets)) {
                    $assets += [ordered]@{
                        name                 = $a.name
                        size                 = $a.size
                        browser_download_url = $a.browser_download_url
                    }
                }
                $rows += [ordered]@{
                    id           = $r.id
                    tag_name     = $r.tag_name
                    name         = $r.name
                    body         = $r.body
                    published_at = $r.published_at
                    created_at   = $r.created_at
                    prerelease   = $r.prerelease
                    html_url     = $r.html_url
                    draft        = $r.draft
                    assets       = @($assets)
                }
            }
            $stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
            $wrapped = ([ordered]@{ time = $stamp; data = @($rows) } | ConvertTo-Json -Depth 8 -Compress)
            [IO.File]::WriteAllText($CachePath, $wrapped, $Utf8NoBom)
            Write-Ok ("快照已刷新（{0} 版，{1:N0} KB）" -f $rows.Count, ((Get-Item $CachePath).Length / 1KB))
        } else {
            Write-Warn2 "快照刷新跳过（GitHub API 不可达或返回非数组），保留旧快照"
        }
    } catch {
        Write-Warn2 "快照刷新异常（保留旧快照）：$($_.Exception.Message)"
    }

    Write-Step "推送清单到 $UpdatesRepo"
    # 注意：必须用 curl.exe + --data-urlencode。PowerShell 的 Invoke-RestMethod
    # 走表单编码会把 base64 里的 "+" 变成空格，Gitee 直接回 406（实测）。
    $B64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Json))
    $sha = $null
    try {
        $cur = Invoke-RestMethod -Uri "$Api/$($UpdatesRepo)/contents/update.json?access_token=$Token" -TimeoutSec 30
        $sha = $cur.sha
    } catch { $sha = $null }

    $method = if ($sha) { 'PUT' } else { 'POST' }
    $curlArgs = @(
        '-sS', '--max-time', '60', '-X', $method, "$Api/$($UpdatesRepo)/contents/update.json",
        '--data-urlencode', "access_token=$Token",
        '--data-urlencode', "content=$B64",
        '--data-urlencode', "message=chore: update.json $Tag",
        '-d', 'branch=master'
    )
    if ($sha) { $curlArgs += @('--data-urlencode', "sha=$sha") }
    $r = Invoke-Native 'curl.exe' $curlArgs
    $resp = ($r.Output -join "`n")
    if ($r.Code -eq 0 -and ($resp -match '"sha"')) {
        Write-Ok "清单已推送（$method）"
    } else {
        Write-Warn2 "清单推送失败（不阻断发版）：$resp"
    }

    # 站点清单要真正上线，必须提交并推送 gh-pages（Pages 约 1 分钟生效）
    if ($PublishSite) {
        Write-Step "提交并推送站点（gh-pages）"
        $ghFile = 'E:\DSH-KF\gh-token.txt'
        $gh = if (Test-Path $ghFile) { (Get-Content $ghFile -Raw).Trim() } else { '' }
        if (-not $gh) {
            Write-Warn2 "缺 gh-token.txt，跳过站点推送（清单已写在工作树）"
        } elseif (-not (Test-Path (Join-Path $SiteRepoDir '.git'))) {
            Write-Warn2 "站点工作树不是 git 仓库，跳过推送"
        } else {
            & git -C $SiteRepoDir add update.json releases/cache.json | Out-Null
            $diff = & git -C $SiteRepoDir status --porcelain -- update.json releases/cache.json
            if ($diff) {
                & git -C $SiteRepoDir commit -m "chore: update.json + releases snapshot $Tag" -- update.json releases/cache.json | Out-Null
                $pushR = Invoke-Native 'git' @('-C', $SiteRepoDir, '-c', 'credential.helper=', 'push', "https://LeafHW:$gh@github.com/LeafHW/hufu-ime-rust.git", 'gh-pages')
                if ($pushR.Code -eq 0) {
                    Write-Ok "站点已推送（Pages 约 1 分钟生效）"
                } else {
                    Write-Warn2 "站点推送失败（不阻断发版）：$($pushR.Output | Select-Object -Last 2 | Out-String)"
                }
            } else {
                Write-Ok "站点清单无变化，跳过提交"
            }
        }
    }
}

# ---------------------------------------------------------------------------
# 5. 清理旧版本附件（保留最近 $KeepReleases 版；1 GB 配额 ≈ 38 版）
# ---------------------------------------------------------------------------
if (-not $DryRun) {
    Write-Step "清理旧附件（保留最近 $KeepReleases 版）"
    try {
        $all = Invoke-RestMethod -Uri "$Api/$Repo/releases?access_token=$Token&per_page=100" -TimeoutSec 40
        $sorted = @($all | Sort-Object { [datetime]$_.created_at } -Descending)
        $drop = @($sorted | Select-Object -Skip $KeepReleases)
        if ($drop.Count -eq 0) {
            Write-Ok "没有需要清理的旧版本（当前 $($sorted.Count) 个）"
        }
        foreach ($rel in $drop) {
            foreach ($a in @($rel.assets)) {
                if ($a.name -notlike '*.exe') { continue }
                try {
                    Invoke-RestMethod -Method Delete -Uri "$Api/$Repo/releases/$($rel.id)/attach_files/$($a.id)?access_token=$Token" -TimeoutSec 30 | Out-Null
                    Write-Ok "已删除 $($rel.tag_name)/$($a.name)"
                } catch {
                    Write-Warn2 "删除失败 $($rel.tag_name)/$($a.name)：$($_.Exception.Message)"
                }
            }
        }
    } catch {
        Write-Warn2 "清理阶段跳过：$($_.Exception.Message)"
    }
}

Write-Step "完成：$Tag"
Write-Host "  清单：https://gitee.com/$Owner/$UpdatesRepo/raw/master/update.json" -ForegroundColor DarkGray
Write-Host "  下载：https://gitee.com/$Owner/$Repo/releases/download/$Tag/$ExeName" -ForegroundColor DarkGray
exit 0
