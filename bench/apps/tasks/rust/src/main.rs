mod model;
mod store;
mod view;

use axum::{body::{Body, to_bytes}, extract::{Request, State}, http::{StatusCode, header}, response::{IntoResponse, Response, Sse}, routing::any, Router};
use datastar::prelude::{PatchElements, PatchSignals};
use model::*;
use std::{collections::HashMap, convert::Infallible, env, net::IpAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::{io::{AsyncBufReadExt, BufReader}, sync::{Mutex, RwLock, mpsc, oneshot, watch}};

#[derive(Clone)]
struct Logger { minimum: u8, wall: bool }
impl Logger {
    fn note(&self, level: u8, message: &str, fields: serde_json::Value) {
        if level < self.minimum { return; }
        let fields:Vec<_>=fields.as_object().unwrap().iter().map(|(name,value)|serde_json::json!({"name":name,"value":value})).collect();
        let value=serde_json::json!({"level":(["debug","info","warn","error"][level as usize]),"message":message,"fields":fields,"timestamp":if self.wall {chrono::Utc::now().timestamp_nanos_opt()} else {None}});
        eprintln!("{value}");
    }
    fn today(&self) -> Option<chrono::NaiveDate> { self.wall.then(|| chrono::Utc::now().date_naive()) }
}
struct Save { board: Board, reply: oneshot::Sender<bool> }
struct Asset { bytes: Vec<u8>, media: &'static str, etag: String }
struct App {
    board: RwLock<Board>, mutation: Mutex<()>, saves: mpsc::Sender<Save>,
    updates: watch::Sender<u64>, stop: watch::Sender<bool>,
    assets: HashMap<String,Asset>, logger: Logger,
}
enum Change { Add(Item), Replace(u64,Item), Next(u64), Drop(u64), Shift(u64,bool) }
impl App {
    async fn commit(&self, change: Change) -> Result<(),u16> {
        let _serial=self.mutation.lock().await;
        if *self.stop.borrow() { return Err(503); }
        let mut board=self.board.write().await;
        match change {
            Change::Add(mut item)=> { item.id=board.next_id; board.next_id+=1; board.tasks.push(item); }
            other=> {
                let id=match &other { Change::Replace(id,_)|Change::Next(id)|Change::Drop(id)|Change::Shift(id,_)=>*id, _=>unreachable!() };
                let i=board.tasks.iter().position(|t|t.id==id).ok_or(404u16)?;
                match other {
                    Change::Replace(_,mut item)=> { item.id=id; item.status=board.tasks[i].status; board.tasks[i]=item; }
                    Change::Next(_)=>board.tasks[i].status=board.tasks[i].status.next(),
                    Change::Drop(_)=> { board.tasks.remove(i); }
                    Change::Shift(_,up)=> {
                        let j=if up { i.checked_sub(1) } else { (i+1<board.tasks.len()).then_some(i+1) };
                        let Some(j)=j else { return Ok(()); }; board.tasks.swap(i,j);
                    }
                    _=>unreachable!()
                }
            }
        }
        board.revision+=1; let revision=board.revision; let snapshot=board.clone(); drop(board);
        let (reply,done)=oneshot::channel();
        self.saves.send(Save {board:snapshot,reply}).await.map_err(|_|503u16)?;
        if !done.await.unwrap_or(false) { return Err(500); }
        self.updates.send_replace(revision);
        Ok(())
    }
}
fn response(status: u16, media: &'static str, body: impl Into<Body>) -> Response {
    Response::builder().status(status).header(header::CONTENT_TYPE,media).body(body.into()).unwrap()
}
fn not_found() -> Response { response(404,"text/html; charset=utf-8",view::not_found()) }
fn allowed(method: &str, methods: &'static str) -> Result<(),Response> {
    if methods.split(", ").any(|m|m==method) { Ok(()) } else {
        let mut r=response(405,"text/plain; charset=utf-8","method not allowed\n"); r.headers_mut().insert(header::ALLOW,methods.parse().unwrap()); Err(r)
    }
}
fn signals(text: impl Into<String>) -> Response {
    let event=PatchSignals::new(text.into()).write_as_axum_sse_event();
    let mut r=Sse::new(async_stream::stream! { yield Ok::<_,Infallible>(event); }).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE,"text/event-stream".parse().unwrap()); r
}
fn pairs(raw: &str) -> Vec<(String,String)> { serde_urlencoded::from_str(raw).unwrap_or_default() }
fn param<'a>(pairs: &'a [(String,String)], key: &str) -> &'a str { pairs.iter().find(|(k,_)| k==key).map(|(_,v)|v.as_str()).unwrap_or("") }
async fn dispatch(app: Arc<App>, request: Request) -> Result<Response,Response> {
    let method=request.method().as_str().to_owned();
    let path=request.uri().path(); let parts: Vec<_>=path.split('/').filter(|p|!p.is_empty()).map(str::to_owned).collect();
    let query=pairs(request.uri().query().unwrap_or(""));
    let is_home=parts.is_empty();
    let id=parts.get(1).and_then(|s|s.parse::<u64>().ok()).filter(|id|*id>0);
    if is_home || (parts.len()==2 && parts[0]=="tasks" && id.is_some() && (method=="GET" || method=="HEAD")) {
        allowed(&method,"GET, HEAD")?;
        let selected=if is_home {0} else {id.unwrap()};
        let status=param(&query,"status"); let sort=param(&query,"sort");
        let screen=Screen {selected,filter:Query {status:if ["todo","doing","done"].contains(&status) {status.into()} else {String::new()},tag:param(&query,"tag").into(),text:param(&query,"q").into(),sort:if ["due","priority","title"].contains(&sort) {sort.into()} else {"manual".into()} }};
        let board=app.board.read().await;
        if selected!=0 && board.find(selected).is_none() {return Ok(not_found());}
        let mut r=response(200,"text/html; charset=utf-8",view::page(&board,&screen,app.logger.today()));
        r.headers_mut().insert(header::CACHE_CONTROL,"no-cache".parse().unwrap());return Ok(r);
    }
    if parts.len()==1 && parts[0]=="updates" {
        allowed(&method,"GET")?;
        let screen:Screen=serde_json::from_str(param(&query,"datastar")).unwrap_or_default();
        let mut updates=app.updates.subscribe(); let mut stop=app.stop.subscribe();
        let events=async_stream::stream! {
            loop {
                if *stop.borrow() { break; }
                updates.borrow_and_update();
                let html={ let board=app.board.read().await; view::live(&board,&screen,app.logger.today()) };
                yield Ok::<_,Infallible>(PatchElements::new(html).write_as_axum_sse_event());
                tokio::select! { _=stop.changed()=>break, changed=updates.changed()=>if changed.is_err() {break;} }
            }
        };
        let mut r=Sse::new(events).into_response();
        r.headers_mut().insert(header::CONTENT_TYPE,"text/event-stream".parse().unwrap());
        return Ok(r);
    }
    if parts.len()==2 && parts[0]=="static" {
        allowed(&method,"GET, HEAD")?;
        let asset=app.assets.get(&parts[1]).ok_or_else(not_found)?;
        let cached=request.headers().get(header::IF_NONE_MATCH).is_some_and(|h|h==asset.etag.as_str());
        let mut r=response(if cached {304} else {200},asset.media,if cached {Vec::new()} else {asset.bytes.clone()});
        r.headers_mut().insert(header::ETAG,asset.etag.parse().unwrap());r.headers_mut().insert(header::CACHE_CONTROL,"no-cache".parse().unwrap()); return Ok(r);
    }
    let create=parts.len()==1 && parts[0]=="tasks";
    let item=parts.len()==2 && parts[0]=="tasks" && id.is_some();
    let action=parts.len()==3 && parts[0]=="tasks" && id.is_some();
    if create { allowed(&method,"POST")?; }
    else if item { allowed(&method,"GET, HEAD, POST, DELETE")?; }
    else if action && ["advance","move"].contains(&parts[2].as_str()) { allowed(&method,"POST")?; }
    else {return Ok(not_found());}
    let edit=item && method=="POST";
    let (change,reply)=if create || edit {
        let media=request.headers().get(header::CONTENT_TYPE).and_then(|v|v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
        if edit && !media.starts_with("application/x-www-form-urlencoded") {return Ok(response(415,"text/plain; charset=utf-8","expected a form\n"));}
        let bytes=tokio::time::timeout(Duration::from_secs(30),to_bytes(request.into_body(),65536)).await.map_err(|_| response(408,"text/plain; charset=utf-8","request timed out\n"))?.map_err(|_|response(413,"text/plain; charset=utf-8","body too large\n"))?;
        let draft=if create {
            #[derive(serde::Deserialize)] struct Signals { draft: Draft }
            serde_json::from_slice::<Signals>(&bytes).map(|s|s.draft).map_err(|_|response(400,"text/plain; charset=utf-8","expected the draft signals\n"))?
        } else {
            let text=std::str::from_utf8(&bytes).map_err(|_|response(400,"text/plain; charset=utf-8","expected a form\n"))?;
            let p=pairs(text); Draft {title:param(&p,"title").into(),description:param(&p,"description").into(),priority:param(&p,"priority").into(),due:param(&p,"due").into(),tags:param(&p,"tags").into()}
        };
        let valid=match draft.validate() { Ok(t)=>t, Err(e)=>return Ok(signals(serde_json::json!({"error":e}).to_string())) };
        if create {(Change::Add(valid),Some(r#"{"draft":{"title":"","description":"","priority":"medium","due":"","tags":""},"error":""}"#))}
        else {(Change::Replace(id.unwrap(),valid),Some(r#"{"editing":0,"error":""}"#))}
    } else if item {(Change::Drop(id.unwrap()),None)}
    else if parts[2]=="advance" {(Change::Next(id.unwrap()),None)}
    else {let direction=param(&query,"dir"); if direction!="up" && direction!="down" {return Ok(not_found());} (Change::Shift(id.unwrap(),direction=="up"),None)};
    match app.commit(change).await {
        Ok(())=>Ok(reply.map(signals).unwrap_or_else(|| StatusCode::NO_CONTENT.into_response())),
        Err(status)=>Ok(response(status,"text/plain; charset=utf-8",match status {404=>"no such task\n",503=>"shutting down\n",_=>"could not save\n"}))
    }
}
async fn handle(State(app): State<Arc<App>>, request: Request) -> Response {
    let method=request.method().clone(); let path=request.uri().path().to_owned();
    let mut r=dispatch(app.clone(),request).await.unwrap_or_else(|r|r);
    app.logger.note(1,if path=="/updates" {"stream"} else {"request"},serde_json::json!({"method":method.as_str(),"path":path,"status":r.status().as_u16().to_string()}));
    if method==axum::http::Method::HEAD { *r.body_mut()=Body::empty(); } r
}
fn setting(args: &[(String,String)],flag: &str,key: &str,default: &str)->String {
    args.iter().rev().find(|(k,_)|k==flag).map(|(_,v)|v.clone()).or_else(||env::var(key).ok()).unwrap_or_else(||default.into())
}
fn main() -> Result<(),Box<dyn std::error::Error>> {
    let mut args=Vec::new(); let mut input=env::args().skip(1);
    while let Some(flag)=input.next() {
        if flag=="--help" || flag=="-h" {println!("tasks [--host IP] [--port|-p PORT] [--durability volatile|durable] [--log-level debug|info|warn|error]");return Ok(());}
        let flag=if flag=="-p" {"--port".into()} else {flag};
        if !["--host","--port","--durability","--log-level"].contains(&flag.as_str()) {return Err(format!("unknown option {flag}").into());}
        args.push((flag,input.next().ok_or("missing flag value")?));
    }
    let host:IpAddr=setting(&args,"--host","TASKS_HOST","127.0.0.1").parse()?;
    let port:u16=setting(&args,"--port","TASKS_PORT","8080").parse()?;
    let durability=setting(&args,"--durability","TASKS_DURABILITY","volatile");
    if !["volatile","durable"].contains(&durability.as_str()) {return Err("durability is volatile or durable".into());}
    let level=setting(&args,"--log-level","TASKS_LOG_LEVEL","info");
    let minimum=["debug","info","warn","error"].iter().position(|v|*v==level).ok_or("invalid log level")? as u8;
    let logger=Logger {minimum,wall:env::var("TASKS_WALL_CLOCK").as_deref()!=Ok("off")};
    let store:Arc<dyn store::Store>=match env::var("TASKS_STORE").unwrap_or_else(|_|"files".into()).as_str() {
        "files"=>Arc::new(store::Files{directory:PathBuf::from(env::var("TASKS_DATA").unwrap_or_else(|_|"data".into())),durable:durability=="durable"}),
        "memory"=>Arc::new(store::Memory::default()), _=>return Err("unknown TASKS_STORE".into())
    };
    let board=store.load()?;
    logger.note(1,"loaded",serde_json::json!({"tasks":board.tasks.len().to_string(),"revision":board.revision.to_string()}));
    let static_dir=env::var("TASKS_STATIC").unwrap_or_else(|_|"static".into());
    let mut assets=HashMap::new();
    for entry in std::fs::read_dir(static_dir)? {
        let entry=entry?;if !entry.file_type()?.is_file(){continue;}
        let name=entry.file_name().into_string().map_err(|_|"asset name is not UTF-8")?;
        let bytes=std::fs::read(entry.path())?;
        let hash=bytes.iter().fold(0xcbf29ce484222325u64,|h,b|(h^u64::from(*b)).wrapping_mul(0x100000001b3));
        let media=if name.ends_with(".js"){"text/javascript; charset=utf-8"}else if name.ends_with(".css"){"text/css; charset=utf-8"}else if name.ends_with(".svg"){"image/svg+xml"}else if name.ends_with(".html"){"text/html; charset=utf-8"}else{"application/octet-stream"};
        assets.insert(name,Asset {bytes,media,etag:format!("\"{hash:x}\"")});
    }
    let mut runtime=match env::var("TASKS_RUNTIME").unwrap_or_else(|_|"current_thread".into()).as_str() {
        "current_thread"=>tokio::runtime::Builder::new_current_thread(),
        "multi_thread"=> {let mut b=tokio::runtime::Builder::new_multi_thread();b.worker_threads(1);b}, _=>return Err("TASKS_RUNTIME is current_thread or multi_thread".into())
    };
    runtime.enable_all().build()?.block_on(async move {
        let (saves,mut jobs)=mpsc::channel::<Save>(64);
        let (updates,_)=watch::channel(board.revision); let (stop,_)=watch::channel(false);
        let app=Arc::new(App {board:RwLock::new(board),mutation:Mutex::new(()),saves,updates,stop:stop.clone(),assets,logger:logger.clone()});
        let writer=tokio::task::spawn_blocking(move || {while let Some(job)=jobs.blocking_recv(){let _=job.reply.send(store.save(&job.board).is_ok());}});
        let listener=tokio::net::TcpListener::bind((host,port)).await?;
        let address=listener.local_addr()?;println!("listening on http://{address}/");
        logger.note(1,"listening",serde_json::json!({"address":address.to_string()}));
        let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let shutdown=async move {
            let stdin=async {let mut lines=BufReader::new(tokio::io::stdin()).lines();while let Ok(Some(line))=lines.next_line().await {if line=="quit"{break;}}};
            tokio::select!{_=stdin=>(),_=tokio::signal::ctrl_c()=>(),_=term.recv()=>()}
            logger.note(1,"stopping",serde_json::json!({"reason":"shutdown"}));stop.send_replace(true);
        };
        let result=axum::serve(listener,Router::new().fallback(any(handle)).with_state(app.clone())).with_graceful_shutdown(shutdown).await;
        drop(app);writer.await?;
        result?;
        Ok::<_,Box<dyn std::error::Error>>(())
    })?;
    // No background writer or connection remains at this point.
    Logger{minimum,wall:env::var("TASKS_WALL_CLOCK").as_deref()!=Ok("off")}.note(1,"stopped",serde_json::json!({}));
    Ok(())
}
