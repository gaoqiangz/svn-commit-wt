# svn-commit-wt
SVN代码提交记录同步到Worktile
# 环境
> rust: 1.44.0-nightly (94d346360 2020-04-09)
> toolchain: nightly-x86_64-pc-windows-msvc
> openssl: 1.1.1d
> VisualSVN Server: 3.6.4
# 静态链接openssl与VC CRT
## 配置`.cargo/config`
```
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]

[target.x86_64-pc-windows-msvc.openssl]
# 指定为static lib路径
rustc-link-search=["native=C:\\Program Files\\OpenSSL-Win64\\lib\\VC\\static"]
```
为了方便使用，`bin\SvnCommitWT.exe`为已经编译的程序。 (链接为VC2015的运行时)
# 部署说明
## 1. 配置ClientId和ClientSecret
1）进入Worktile研发版的`企业后台管理` > `应用管理` > `自定义应用`。
2）新建应用，输入`应用名`，将`DevOps：开发`的权限设置为`读写`，点击确定。
3）在应用列表中找到创建的应用，分别复制`ClientID`和`Secret`。
4）回到服务器
更新`config.toml`配置里的`client_id`和`client_secret`：
```
[worktile]
client_id = "自定义应用的CLIENT_ID"
client_secret = "自定义应用的CLIENT_SECRECT"
```
## 2. 配置VisualSVN Server的[Post-commit hook]，将svn的提交信息同步到Worktile中
```
SET "SVNCWT=D:\Program Files\svn_commit_wt\SvnCommitWT.exe"

"%SVNCWT%" commit -p "%1" -n "JMP" -r "%2"
```
## 3. 并`SvnCommitWT`注册为Windows服务并启动
```
SvnCommitWT service --install
SvnCommitWT service --start
```
## 4. `SvnCommitWT`支持的命令行参数
[service]命令
```
--install      安装为Windows服务
--uninstall    从Windows服务卸载
--start        开始Windows服务
--stop         停止Windows服务
--run          直接运行服务
```
[commit]命令
```
--repo_name <repo_name> --repo_path <repo_path> --revision <revision>
```
### 客户端提交代码
向代码仓库提交代码，commit message中提及Worktile的工作项即可，例如：
```
svn commit -m 'feat(scope): #CD-7 some comment'
```
这里的`CD-7`是Worktile工作项（史诗、特性、用户故事、任务、缺陷）的编号，在Worktile中点开某一个工作项即可在左上角找到工作项编号。
