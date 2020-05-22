//!
//! Worktile接口封装
//! https://open.worktile.com
//!

#![allow(dead_code)]
use super::AnyError;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json as json;
use std::{
    collections::HashMap, sync::{Arc, RwLock}
};

const DEFAULT_API_URL: &'static str = "https://open.worktile.com";

pub struct ClientBuilder {
    api_url: String,
    product_name: Option<String>,
    id: Option<String>,
    key: Option<String>
}

impl ClientBuilder {
    pub fn new() -> ClientBuilder {
        ClientBuilder {
            api_url: DEFAULT_API_URL.to_owned(),
            product_name: None,
            id: None,
            key: None
        }
    }
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }
    pub fn product_name(mut self, name: impl Into<String>) -> Self {
        self.product_name = Some(name.into());
        self
    }
    pub fn credential(mut self, id: impl Into<String>, key: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self.key = Some(key.into());
        self
    }
    pub fn build(self) -> Result<Client, AnyError> {
        if self.api_url.is_empty() {
            return Err("API地址为空".into());
        }
        let product_name = match self.product_name {
            Some(name) if !name.is_empty() => name,
            _ => return Err("[product_name]未指定".into())
        };
        let (id, key) = match (self.id, self.key) {
            (Some(id), Some(key)) if !id.is_empty() && !key.is_empty() => (id, key),
            _ => return Err("API认证信息为空".into())
        };
        Ok(Client {
            client: reqwest::ClientBuilder::new().danger_accept_invalid_certs(true).no_proxy().build()?,
            api_url: self.api_url,
            product_name,
            id,
            key,
            ctx: Arc::new(RwLock::new(Context::new()))
        })
    }
}

#[derive(Clone)]
pub struct Client {
    client: reqwest::Client,
    api_url: String,
    /// 代码托管平台名称
    product_name: String,
    /// CLIENT_ID
    id: String,
    /// CLIENT_SECRET
    key: String,
    /// 接口的上下文信息
    ctx: Arc<RwLock<Context>>
}

/// 接口的上下文信息
struct Context {
    /// 访问令牌
    access_token: Option<AccessToken>,
    /// 代码托管平台的ID
    product_id: Option<String>,
    /// 代码托管平台的用户ID列表
    users: HashMap<String, String>,
    /// 代码仓库的ID列表
    repositories: HashMap<String, String>,
    /// 代码仓库的分支ID列表
    branches: HashMap<(String, String), String>
}

impl Context {
    fn new() -> Context {
        Context {
            access_token: None,
            product_id: None,
            users: HashMap::new(),
            repositories: HashMap::new(),
            branches: HashMap::new()
        }
    }
}

/// 访问令牌
#[derive(Debug, Deserialize)]
struct AccessToken {
    #[serde(rename = "access_token")]
    token: String,
    #[serde(deserialize_with = "deserialize_ts")]
    expires_in: chrono::NaiveDateTime
}

/// 令牌刷新的时间阈值(小时)
const TOKEN_REFRESH_THRESOLD: i64 = 12;

/// 提取响应数据的[Id]值
#[derive(Deserialize)]
struct ExtractId {
    id: String
}

/// 提取响应数据的[Id]列表
#[derive(Deserialize)]
struct ExtractIds {
    values: Vec<ExtractId>
}

/// 提交信息的元数据
#[derive(Debug)]
pub struct CommitMeta {
    pub sha: String,
    pub message: String,
    pub committer_name: String,
    pub committed_at: chrono::NaiveDateTime,
    pub files_added: Vec<String>,
    pub files_removed: Vec<String>,
    pub files_modified: Vec<String>
}

impl Client {
    pub fn build() -> ClientBuilder { ClientBuilder::new() }

    /// 提交代码
    pub async fn commit(
        &self,
        repo: impl AsRef<str>,
        branch: impl AsRef<str>,
        meta: CommitMeta
    ) -> Result<(), AnyError> {
        let prod_id = self.product_id().await?;
        let repo_id = self.repository_id(repo.as_ref()).await?;
        let branch_id = self.branch_id(repo.as_ref(), branch.as_ref()).await?;

        //确保用户存在于Worktile
        let _user_id = self.user_id(&meta.committer_name).await?;

        //创建提交
        let _: ExtractId = self
            .http_post(
                "v1/scm/commits",
                json::json!({
                    "sha": meta.sha,
                    "message": meta.message,
                    "committer_name": meta.committer_name,
                    "committed_at": meta.committed_at.timestamp(),
                    "tree_id": tree_id(&repo_id,&branch_id)?,
                    "files_added": meta.files_added,
                    "files_removed": meta.files_removed,
                    "files_modified": meta.files_modified,
                    "work_item_identifiers": identifiers_from_message(&meta.message)?
                })
            )
            .await?;

        //创建引用
        let _: ExtractId = self
            .http_post(
                format!("v1/scm/products/{}/repositories/{}/refs", prod_id, repo_id),
                json::json!({
                    "meta_type": "branch",
                    "meta_id": branch_id,
                    "sha": meta.sha
                })
            )
            .await?;

        Ok(())
    }

    /// 获取代码托管平台ID
    async fn product_id(&self) -> Result<String, AnyError> {
        if let Some(id) = &self.ctx.read().unwrap().product_id {
            return Ok(id.to_owned());
        }

        //查询
        let products: ExtractIds =
            self.http_get(format!("v1/scm/products?name={}", self.product_name)).await?;
        if !products.values.is_empty() {
            let id = products.values[0].id.to_owned();
            self.ctx.write().unwrap().product_id = Some(id.to_owned());
            return Ok(id);
        }

        //创建
        let product: ExtractId = self
            .http_post(
                "v1/scm/products",
                json::json!({
                    "name": self.product_name,
                    "type": "svn",
                     "description": "Subversion"
                })
            )
            .await?;

        self.ctx.write().unwrap().product_id = Some(product.id.to_owned());

        Ok(product.id)
    }

    /// 获取代码托管平台的用户ID列表
    async fn user_id(&self, name: impl AsRef<str>) -> Result<String, AnyError> {
        if let Some(id) = self.ctx.read().unwrap().users.get(name.as_ref()) {
            return Ok(id.to_owned());
        }

        let prod_id = self.product_id().await?;

        //查询
        let users: ExtractIds =
            self.http_get(format!("v1/scm/products/{}/users?name={}", prod_id, name.as_ref())).await?;
        if !users.values.is_empty() {
            let id = users.values[0].id.to_owned();
            self.ctx.write().unwrap().users.insert(name.as_ref().to_owned(), id.to_owned());
            return Ok(id);
        }

        //创建
        let user: ExtractId = self
            .http_post(
                format!("v1/scm/products/{}/users", prod_id),
                json::json!({
                    "name": name.as_ref(),
                    "display_name": name.as_ref(),
                })
            )
            .await?;

        self.ctx.write().unwrap().users.insert(name.as_ref().to_owned(), user.id.to_owned());

        Ok(user.id)
    }

    /// 获取代码仓库ID
    async fn repository_id(&self, name: impl AsRef<str>) -> Result<String, AnyError> {
        if let Some(id) = self.ctx.read().unwrap().repositories.get(name.as_ref()) {
            return Ok(id.to_owned());
        }

        let prod_id = self.product_id().await?;

        //查询
        let repos: ExtractIds = self
            .http_get(format!("v1/scm/products/{}/repositories?full_name={}", prod_id, name.as_ref()))
            .await?;
        if !repos.values.is_empty() {
            let id = repos.values[0].id.to_owned();
            self.ctx.write().unwrap().repositories.insert(name.as_ref().to_owned(), id.to_owned());
            return Ok(id);
        }

        //创建
        let repo: ExtractId = self
            .http_post(
                format!("v1/scm/products/{}/repositories", prod_id),
                json::json!({
                    "name": name.as_ref(),
                    "full_name": name.as_ref(),
                    "is_fork": false,
                    "is_private": true,
                    "owner_name": "admin",
                    "created_at": chrono::Utc::now().timestamp()
                })
            )
            .await?;

        self.ctx.write().unwrap().repositories.insert(name.as_ref().to_owned(), repo.id.to_owned());

        Ok(repo.id)
    }

    /// 获取代码仓库的分支ID列表
    async fn branch_id(&self, repo: impl AsRef<str>, name: impl AsRef<str>) -> Result<String, AnyError> {
        let key = (repo.as_ref().to_owned(), name.as_ref().to_owned());
        if let Some(id) = self.ctx.read().unwrap().branches.get(&key) {
            return Ok(id.to_owned());
        }

        let prod_id = self.product_id().await?;
        let repo_id = self.repository_id(repo.as_ref()).await?;

        //查询
        let branches: ExtractIds = self
            .http_get(format!(
                "v1/scm/products/{}/repositories/{}/branches?name={}",
                prod_id,
                repo_id,
                name.as_ref()
            ))
            .await?;
        if !branches.values.is_empty() {
            let id = branches.values[0].id.to_owned();
            self.ctx.write().unwrap().branches.insert(key, id.to_owned());
            return Ok(id);
        }

        //创建
        let branch: ExtractId = self
            .http_post(
                format!("v1/scm/products/{}/repositories/{}/branches", prod_id, repo_id,),
                json::json!({
                    "name": name.as_ref(),
                    "sender_name": "admin",
                    "created_at": chrono::Utc::now().timestamp(),
                })
            )
            .await?;

        self.ctx.write().unwrap().branches.insert(key, branch.id.to_owned());

        Ok(branch.id)
    }

    /// 获取访问令牌
    async fn access_token(&self) -> Result<String, AnyError> {
        if let Some(access_token) = &self.ctx.read().unwrap().access_token {
            return Ok(access_token.token.to_owned());
        }

        let mut ctx = self.ctx.write().unwrap();

        //防止并发时重复刷新令牌
        if let Some(access_token) = &ctx.access_token {
            if access_token.expires_in.signed_duration_since(chrono::Utc::now().naive_utc()).num_hours() >
                TOKEN_REFRESH_THRESOLD
            {
                return Ok(access_token.token.to_owned());
            }
        }

        let uri = format!(
            "v1/auth/token?grant_type=client_credentials&client_id={}&client_secret={}",
            self.id, self.key
        );

        ctx.access_token = None;
        ctx.access_token =
            Some(json::from_value(self.http_request_impl(reqwest::Method::GET, &uri, None, None).await?)?);

        Ok(ctx.access_token.as_ref().unwrap().token.to_owned())
    }

    /// 发起HTTP GET请求
    async fn http_get<R>(&self, uri: impl AsRef<str>) -> Result<R, AnyError>
    where
        R: DeserializeOwned
    {
        self.http_request(reqwest::Method::GET, uri, None).await
    }

    /// 发起HTTP POST请求
    async fn http_post<R>(&self, uri: impl AsRef<str>, body: json::Value) -> Result<R, AnyError>
    where
        R: DeserializeOwned
    {
        self.http_request(reqwest::Method::POST, uri, body).await
    }

    /// 发起HTTP请求
    async fn http_request<R>(
        &self,
        method: reqwest::Method,
        uri: impl AsRef<str>,
        body: impl Into<Option<json::Value>>
    ) -> Result<R, AnyError>
    where
        R: DeserializeOwned
    {
        let body = body.into();
        let mut tried = false;
        let resp = loop {
            let access_token = self.access_token().await?;
            let mut headers = reqwest::header::HeaderMap::new();
            headers.append("authorization", format!("Bearer {}", access_token).parse()?);
            let resp =
                self.http_request_impl(method.clone(), uri.as_ref(), body.as_ref(), Some(headers)).await?;
            // 检查响应结果判断是否需要刷新令牌
            if let Some(code) =
                resp.as_object().and_then(|obj| obj.get("code")).and_then(|code| code.as_str())
            {
                //100026 	'access_token'无效
                //100028 	'access_token'已失效
                //100032 	'authorization_code'鉴权失败
                if (code == "100028" || code == "100032") && !tried {
                    let mut ctx = self.ctx.write().unwrap();
                    if let Some(token) = &ctx.access_token {
                        //检查令牌是否发生了改变，防止并发
                        if token.token == access_token {
                            //清空当前的令牌，使强制刷新
                            ctx.access_token = None;
                        }
                    }
                    tried = true;
                    continue;
                } else {
                    return Err(format!("Worktile API error code: {}", code).into());
                }
            }
            break resp;
        };
        json::from_value(resp).map_err(|e| e.into())
    }

    /// 发起HTTP请求
    async fn http_request_impl(
        &self,
        method: reqwest::Method,
        uri: &str,
        body: Option<&json::Value>,
        headers: Option<reqwest::header::HeaderMap>
    ) -> Result<json::Value, AnyError> {
        let url = format!("{}/{}", self.api_url, uri);
        let mut req = self.client.request(method.clone(), &url);
        req = match headers {
            Some(headers) => req.headers(headers),
            None => req
        };
        req = match body {
            Some(body) => req.json(body),
            None => req
        };
        info!(
            "HTTP {} {}, {}",
            method.as_ref(),
            url,
            body.map(|v| v.to_string()).unwrap_or("NULL".to_owned())
        );
        let resp = req.send().await?;
        if resp.status().is_success() {
            info!("{:?}", resp);
        } else {
            warn!("{:?}", resp);
        }
        let resp: json::Value = resp.json().await?;
        info!("Response JSON, Url: {}, Body: {}", url, resp);
        Ok(resp)
    }
}

/// 计算提交的树ID
/// SHA(repo/branch)
fn tree_id(repo: &str, branch: &str) -> Result<String, AnyError> {
    use openssl::hash::{Hasher, MessageDigest};

    let mut hasher = Hasher::new(MessageDigest::sha1())?;
    hasher.update(format!("{}/{}", repo, branch).as_bytes())?;
    Ok(hex::encode(hasher.finish()?))
}

/// 从提交的Message里提取关联的Worktile工作项编号
/// 如: #PROD-1234
fn identifiers_from_message(message: &str) -> Result<Vec<String>, AnyError> {
    use regex::Regex;

    let re = Regex::new(r"(?m)#[^\s]*[A-Za-z0-9_]+-[0-9]+")?;
    let mut identifiers = Vec::new();
    for item in re.find_iter(message) {
        identifiers.push(item.as_str()[1..].to_owned());
    }
    Ok(identifiers)
}

/// 解析Timestamp数值(秒)
fn deserialize_ts<'de, D>(deserializer: D) -> Result<chrono::NaiveDateTime, D::Error>
where
    D: serde::Deserializer<'de>
{
    let ts = i64::deserialize(deserializer)?;
    Ok(chrono::NaiveDateTime::from_timestamp(ts, 0))
}
