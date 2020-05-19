//!
//! HTTP接口服务
//!

use super::*;
use actix_rt::{Arbiter, System};
use actix_web::*;
use futures::future;
use serde::Deserialize;
use serde_json as json;
use std::sync::{Arc, Mutex};

/// 调用本地HTTP服务提交代码记录
pub fn request_commit(repo_path: &str, repo_name: &str, rev: &str) -> Result<(), AnyError> {
    let cfg = settings::SharedConfig::load()?;
    let addr = cfg.config_string("http.listen");
    let port = addr.split(":").skip(1).next().unwrap_or("80");
    reqwest::blocking::Client::new()
        .post(&format!("http://127.0.0.1:{}/commit", port))
        .json(&json::json!({
            "repo_path": repo_path,
            "repo_name": repo_name,
            "rev": rev
        }))
        .send()?;
    Ok(())
}

/// 启动HTTP服务
pub fn http_serve(stop_signer: Option<oneshot::Receiver<()>>) -> Result<u16, AnyError> {
    let cfg = settings::SharedConfig::load()?;

    //Worktile客户端接口
    let wt = worktile::Client::build()
        .product_name(cfg.config_string("worktile.product_name"))
        .credential(cfg.config_string("worktile.client_id"), cfg.config_string("worktile.client_secret"))
        .build()?;

    //创建Actix运行时
    let mut system = System::new("main");
    let arbiter = Arbiter::current();

    let http_srv: Arc<Mutex<Option<dev::Server>>> = Arc::new(Mutex::new(None));

    //如果有停止信号的通道则监听事件(由SCM触发)
    if let Some(stop) = stop_signer {
        let http_srv = http_srv.clone();
        arbiter.send(Box::pin(async move {
            if stop.await.is_ok() {
                info!("STOP sign received");
                if let Some(http_srv) = http_srv.lock().unwrap().take() {
                    let _ = http_srv.stop(true);
                }
            }
        }));
    }

    //创建HTTP服务
    let addr = cfg.config_string("http.listen");
    let srv = HttpServer::new(move || {
        App::new()
            .app_data(wt.clone())
            .wrap(middleware::NormalizePath)
            .wrap(middleware::Compress::default())
            .wrap(middleware::Logger::new(&cfg.config_string("http.log_format")))
            .service(commit)
    })
    .bind(addr)
    .map_err(|e| format!("http server bind failed: {}", e))?
    .run();

    //将Server保存在全局的Application中，用于退出
    *http_srv.lock().unwrap() = Some(srv.clone());

    //进入HTTP服务事件循环
    let http_srv_exit_status = system.block_on(srv);

    http_srv_exit_status?;

    Ok(win_service::exit_code::OK)
}

impl FromRequest for worktile::Client {
    type Config = ();
    type Error = ();
    type Future = future::Ready<Result<Self, Self::Error>>;

    #[inline]
    fn from_request(req: &HttpRequest, _: &mut dev::Payload) -> Self::Future {
        future::ok(req.app_data::<worktile::Client>().unwrap().clone())
    }
}

/// 代码提交请求
#[derive(Debug, Deserialize)]
struct CommitParams {
    repo_path: String,
    repo_name: String,
    rev: String
}

#[post("/commit")]
async fn commit(wt: worktile::Client, params: web::Json<CommitParams>) -> HttpResponse {
    let meta = match commit_meta_from_svn(&params.repo_path, &params.rev).await {
        Ok(meta) => meta,
        Err(e) => {
            return HttpResponse::BadRequest().json(json::json!({
                "status": -1,
                "msg": e.to_string()
            }));
        }
    };
    let branch = match svn::commit_branch(&params.repo_path, &params.rev).await {
        Ok(branch) => {
            //默认分支为trunk
            branch.unwrap_or("trunk".to_owned())
        },
        Err(e) => {
            return HttpResponse::BadRequest().json(json::json!({
                "status": -1,
                "msg": e.to_string()
            }));
        }
    };

    //异步提交
    actix_rt::spawn(async move {
        let _ = wt.commit(&params.repo_name, &branch, meta).await;
    });

    HttpResponse::Ok().json(json::json!({
        "status": 0,
        "msg": "成功"
    }))
}

/// 从SVN提交记录里提取Worktile需要的元数据
async fn commit_meta_from_svn(repo_path: &str, rev: &str) -> Result<worktile::CommitMeta, AnyError> {
    use rand::{thread_rng, Rng};

    //填充随机数满足40位SHA值
    //0-9 a-f
    let sha = format!(
        "{}{}{}",
        rev,
        " ".repeat(10 - rev.len().min(10)),
        hex::encode((0..15).map(|_| thread_rng().gen::<u8>()).collect::<Vec<u8>>())
    );
    let files_changed = svn::commit_changed(repo_path, rev).await?;
    let meta = worktile::CommitMeta {
        sha,
        message: svn::commit_message(repo_path, rev).await?,
        committer_name: svn::commit_author(repo_path, rev).await?,
        committed_at: svn::commit_date(repo_path, rev).await?,
        files_added: files_changed.added,
        files_removed: files_changed.removed,
        files_modified: files_changed.modified
    };

    Ok(meta)
}
