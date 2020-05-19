//!
//! SVN提交记录提取封装
//!

use super::AnyError;
use encoding::{all::GBK as CMDCS, DecoderTrap, Encoding};
use tokio::process::Command;

pub async fn commit_message(repo_path: &str, rev: &str) -> Result<String, AnyError> {
    svnlook(&["log", repo_path, "-r", rev]).await
}

pub async fn commit_author(repo_path: &str, rev: &str) -> Result<String, AnyError> {
    svnlook(&["author", repo_path, "-r", rev]).await
}

pub async fn commit_date(repo_path: &str, rev: &str) -> Result<chrono::NaiveDateTime, AnyError> {
    let mut date = svnlook(&["date", repo_path, "-r", rev]).await?;
    //svnlook date返回的日期格式为： 2020-05-17 14:27:23 +0800 (周日, 17 5月 2020)
    date.truncate(date.find(" (").unwrap_or(date.len()));
    chrono::DateTime::parse_from_str(&date, "%Y-%m-%d %H:%M:%S %z")
        .map(|dtt| dtt.naive_utc())
        .map_err(|e| format!("解析日期: {}, 失败: {}", date, e).into())
}

pub async fn commit_branch(repo_path: &str, rev: &str) -> Result<Option<String>, AnyError> {
    use regex::Regex;

    let changed = svnlook(&["dirs-changed", repo_path, "-r", rev]).await?;

    //提取branches和tags路径的分支名称
    let re = Regex::new(r"(?m).*/(?:branches|tags)/(\w+)/.*")?;
    if let Some(branches) = re.captures(&changed) {
        if let Some(branch) = branches.get(1) {
            return Ok(Some(branch.as_str().to_owned()));
        }
    }

    return Ok(None);
}

pub struct FilesChanged {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>
}

pub async fn commit_changed(repo_path: &str, rev: &str) -> Result<FilesChanged, AnyError> {
    let changed = svnlook(&["changed", repo_path, "-r", rev]).await?;
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();
    for line in changed.split("\r\n") {
        let mut target = None;
        if line.starts_with("A") {
            target = Some(&mut added);
        } else if line.starts_with("D") {
            target = Some(&mut removed);
        } else if line.starts_with("U") {
            target = Some(&mut modified);
        }
        if let Some(target) = target {
            let mut line = line[1..].trim_start().to_owned();
            //去除结尾的/
            if line.ends_with('/') {
                line.pop();
            }
            target.push(line);
        }
    }
    Ok(FilesChanged {
        added,
        removed,
        modified
    })
}

async fn svnlook(args: &[&str]) -> Result<String, AnyError> {
    let output = Command::new("svnlook").args(args).output().await?;
    if output.status.success() {
        let mut rv = CMDCS.decode(&output.stdout, DecoderTrap::Replace)?;
        //去除结尾的\r\n
        if rv.ends_with('\n') {
            rv.pop();
            if rv.ends_with('\r') {
                rv.pop();
            }
        }
        Ok(rv)
    } else {
        let buf = if output.stderr.len() > 0 {
            &output.stderr
        } else {
            &output.stdout
        };
        let err = CMDCS.decode(buf, DecoderTrap::Replace)?;
        let mut err = err.trim();
        if err.is_empty() {
            err = "(EMPTY)";
        }
        warn!("svnlook {}, stderr: {}", args.join(" "), err);
        Err(format!("svnlook {}, {}", args.join(" "), err).into())
    }
}
