# 虎符 HuFu · 官网

虎符输入法（[LeafHW/hufu-ime-rust](https://github.com/LeafHW/hufu-ime-rust)）的静态官网，部署在 GitHub Pages：

**<https://leafhw.github.io/hufu-ime-rust/>**

## 本地预览

```sh
cd hufu-site
python -m http.server 8788 --bind 127.0.0.1
# 打开 http://127.0.0.1:8788/
```

站点是纯静态 HTML/CSS/JS，没有构建步骤；修改文件后刷新即可。

## 目录

```text
hufu-site/
├─ index.html                 首页
├─ 404.html                   404（base 指向 /hufu-ime-rust/）
├─ assets/
│  ├─ css/main.css            设计系统 + 全部样式
│  ├─ js/main.js              主题、动效、皮肤实验室、下载、灯箱等
│  ├─ fonts/                  Noto Serif SC 子集 + OFL 授权
│  └─ img/                    Logo、截图 WebP、OG 图、favicon
├─ robots.txt / sitemap.xml
├─ site.webmanifest
└─ .nojekyll                  GitHub Pages 关闭 Jekyll
```

## 内容与素材

- **文案与数据**：来自仓库 README、`docs/features.md`、`docs/status.md` 与 v1.9.8 发布说明。
- **截图**：设置页使用当前版本「概览（Ctrl+K 搜索）/ 云同步 / 键位编辑」截图（来自 `hufu/settings-app/screenshots/`），皮肤与整句截图来自 `docs/design/` 等；已转 WebP，仅作界面示意。
- **首页候选窗动画**：用 HTML/CSS 复刻真实候选窗（编码 `syfdgvwlkuvnagj` → 整句「让中文收指成句」），不是视频；页面内已标注“网页示意”。
- **下载信息**：页面加载后从 GitHub Releases API 读取最新版本号、日期、exe 直链与大小；API 不可用时回落到静态 v1.9.8 链接，不影响访问。
- **字体**：Noto Serif SC 子集（仅包含页面用到的字符），SIL Open Font License 1.1，授权全文见 `assets/fonts/LICENSE.txt`。

## 部署

线上由 **`gh-pages` 分支**发布（本目录内容即该分支内容），GitHub Pages 源为 `gh-pages` / root。

更新步骤（在本机）：

```sh
cd hufu-site
git add -A
git commit -m "site: ..."
git -c credential.helper= push \
  "https://x-access-token:$(cat ../gh-token.txt)@github.com/LeafHW/hufu-ime-rust.git" gh-pages
```

推送 `gh-pages` 分支后，GitHub Pages 通常会自动创建（本仓 2026-11-02 实测：推送后自动变为 `building → built`）。
如未自动发布，到仓库 **Settings → Pages** 选择：

- Source: **Deploy from a branch**
- Branch: **gh-pages** / **/(root)**

> 注：用 fine-grained PAT 调 `POST /repos/{owner}/{repo}/pages` 会返回 403（缺 Pages 权限）；实际不需要调这个 API，推送 `gh-pages` 即可触发。
> 线上已验证：`status=built`、`https_enforced=true`、`custom_404=true`，仓库首页 Website 已指向该地址。

## 说明

- **数据**：万句基准 99.73%（纯 ngram）；叠加 Qwen3 神经重排可达 99.9%。

- 本站代码随虎符项目以 GPL-3.0-or-later 发布；字体为 OFL 1.1；截图与 Logo 版权归虎符项目。
- 页面会读取 `api.github.com` 获取最新版本；如网络受限，会使用静态兜底。
- 主题默认跟随系统，手动切换后会写入 `localStorage`（`hufu-theme`）。
