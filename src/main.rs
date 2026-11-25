#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use eframe::egui::{self, Color32, RichText};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::{Child, Command, Stdio}, sync::mpsc::{self, Sender, Receiver}, time::{Duration, Instant}};
#[cfg(windows)] use std::os::windows::process::CommandExt;
const BASE: &str = "http://127.0.0.1:18639/api/v0";
type R<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String { e.to_string() }
fn s<'a>(v: &'a Value, key: &str) -> &'a str { v[key].as_str().unwrap_or("") }
fn arr(v: &Value) -> &[Value] { v.as_array().map(Vec::as_slice).unwrap_or(&[]) }
fn yes(v: &Value, key: &str) -> bool { v[key].as_bool().unwrap_or(false) }
struct Api(reqwest::blocking::Client);
impl Api {
 fn new() -> R<Self> { Ok(Self(reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build().map_err(err)?)) }
 fn call(&self, path: &str, body: Option<Value>) -> R<String> {
  let u = format!("{BASE}{path}");
  let r = match body { Some(v) => self.0.post(u).json(&v), None => self.0.get(u) }.send().map_err(err)?;
  let status = r.status(); let text = r.text().map_err(err)?;
  if !status.is_success() { return Err(format!("HTTP {status}: {text}")); } Ok(text)
 }
 fn get(&self, p: &str) -> R<Value> { serde_json::from_str(&self.call(p,None)?).map_err(err) }
 fn post(&self,p:&str,v:Value)->R<String>{self.call(p,Some(v))}
}
fn profile() -> R<PathBuf> { Ok(PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA missing")?).join("LiteLAN").join("profile")) }
fn core_path()->R<PathBuf>{ Ok(std::env::current_exe().map_err(err)?.parent().ok_or("No executable directory")?.join("core").join("awl.exe")) }
fn start_core() -> R<Child> {
 let dir=profile()?; fs::create_dir_all(&dir).map_err(err)?;
 let file=dir.join("config_awl.json");
 let mut cfg=if file.exists(){serde_json::from_slice::<Value>(&fs::read(&file).map_err(err)?).map_err(err)?}else{json!({})};
 if !cfg.is_object(){return Err("Invalid profile JSON".into())}
 if file.exists(){fs::copy(&file,dir.join("config_awl.backup.json")).map_err(err)?;}
 for k in ["p2pNode","vpn","vpnGateway","socks5","dns","update","httpBasicAuth"] {if !cfg[k].is_object(){cfg[k]=json!({});}}
 cfg["httpListenAddress"]=json!("127.0.0.1:18639"); cfg["httpListenOnAdminHost"]=json!(false);
 cfg["httpBasicAuth"]=json!({"username":"","password":""});
 cfg["loggerLevel"]=json!("info"); cfg["p2pNode"]["autoAcceptAuthRequests"]=json!(false);
 if s(&cfg["p2pNode"],"name").is_empty(){cfg["p2pNode"]["name"]=json!(std::env::var("COMPUTERNAME").unwrap_or("LiteLAN".into()));}
 cfg["vpn"]["interfaceName"]=json!("litelan0"); cfg["vpn"]["disableVPNInterface"]=json!(false); cfg["vpn"]["mtu"]=json!(1400);
 cfg["vpnGateway"]=json!({"clientEnabled":false,"gatewayPeerID":"","serverEnabled":false});
 cfg["socks5"]["listenerEnabled"]=json!(false); cfg["socks5"]["proxyingEnabled"]=json!(false); cfg["socks5"]["usingPeerID"]=json!("");
 cfg["dns"]["disableDNS"]=json!(true); cfg["update"]["trayAutoCheckEnabled"]=json!(false);
 fs::write(&file,serde_json::to_vec_pretty(&cfg).map_err(err)?).map_err(err)?;
 let exe=core_path()?; if !exe.is_file(){return Err(format!("Core not found: {}",exe.display()));}
 let log=fs::File::create(dir.join("core.log")).map_err(err)?;
 let mut cmd=Command::new(exe); cmd.env("AWL_DATA_DIR",&dir).stdin(Stdio::null()).stdout(Stdio::from(log.try_clone().map_err(err)?)).stderr(Stdio::from(log));
 #[cfg(windows)] cmd.creation_flags(0x08000000);
 cmd.spawn().map_err(err)
}
#[derive(Default)] struct Snapshot {me:Value, peers:Value, requests:Value}
enum Job {Start,Stop,Invite,Add(String,String),Accept(String,String,bool),Rename(String),Logs,Exit}
enum Event {State(String),Data(Snapshot),Invite(String),Logs(String),Error(String)}
fn worker(rx:Receiver<Job>,tx:Sender<Event>,ctx:egui::Context){
 let send=|e|{let _=tx.send(e);ctx.request_repaint();};
 let api=match Api::new(){Ok(a)=>a,Err(e)=>{send(Event::Error(e));return}};
 let mut child:Option<Child>=None; let mut started=Instant::now(); let mut next=Instant::now();
 loop {
  match rx.recv_timeout(Duration::from_millis(200)) {
   Ok(Job::Exit)|Err(mpsc::RecvTimeoutError::Disconnected)=>{stop(&mut child);break},
   Ok(Job::Start)=>{if child.is_none(){
    if std::net::TcpStream::connect_timeout(&"127.0.0.1:18639".parse().unwrap(),Duration::from_millis(300)).is_ok(){send(Event::Error("Port 18639 already in use. Close the other LiteLAN instance.".into()));}
    else {match start_core(){Ok(c)=>{child=Some(c);started=Instant::now();next=Instant::now();send(Event::State("Starting core…".into()));},Err(e)=>send(Event::Error(e))}}
   }},
   Ok(Job::Stop)=>{stop(&mut child);send(Event::State("Stopped".into()));send(Event::Data(Snapshot::default()));},
   Ok(Job::Logs)=>{let text=api.call("/debug/log?logs=100",None).or_else(|_|fs::read_to_string(profile()?.join("core.log")).map_err(err));match text{Ok(t)=>send(Event::Logs(t)),Err(e)=>send(Event::Error(e))}},
   Ok(job)=>{
    let res=match job {
     Job::Invite=>api.post("/peers/invites/create",json!({"maxUses":1,"expiresInSeconds":86400,"allowUsingAsExitNode":false})).and_then(|t|{let v:Value=serde_json::from_str(&t).map_err(err)?;let link=s(&v,"link");if link.is_empty(){return Err("Invite response missing link".into())}send(Event::Invite(link.into()));Ok(String::new())}),
     Job::Add(link,alias)=>parse_invite(&link,&alias).and_then(|v|api.post("/peers/invite_peer",v)),
     Job::Accept(id,alias,decline)=>api.post("/peers/accept_peer",json!({"peerID":id,"alias":alias,"decline":decline,"allowUsingAsExitNode":false})),
     Job::Rename(name)=>api.post("/settings/update",json!({"name":name})),
     _=>Ok(String::new())};
    match res{Ok(_)=>{next=Instant::now();},Err(e)=>send(Event::Error(e))}
   },Err(mpsc::RecvTimeoutError::Timeout)=>{}
  }
  if let Some(c)=child.as_mut(){match c.try_wait(){Ok(Some(status))=>{child=None;send(Event::State(format!("Core exited: {status}. Open logs.")));send(Event::Data(Snapshot::default()));continue},Err(e)=>{send(Event::Error(err(e)));},_=>{}}}
  if child.is_some() && Instant::now()>=next{
   next=Instant::now()+Duration::from_secs(3);
   match api.get("/settings/peer_info").and_then(|me|Ok(Snapshot{me,peers:api.get("/peers/get_known")?,requests:api.get("/peers/auth_requests")?})){
    Ok(s)=>{send(Event::State("Core running".into()));send(Event::Data(s));},
    Err(e)=>{send(Event::Data(Snapshot::default()));send(Event::State(format!("{}: {e}",if started.elapsed()<Duration::from_secs(20){"Starting"}else{"API unavailable — open logs"})));}
   }
  }
 }
}
fn stop(child:&mut Option<Child>){if let Some(mut c)=child.take(){let _=c.kill();let _=c.wait();}}
fn parse_invite(link:&str,alias:&str)->R<Value>{
 let u=url::Url::parse(link.trim()).map_err(err)?;if u.scheme()!="awl"||u.host_str()!=Some("invite"){return Err("Expected awl://invite link".into())}
 let (mut id,mut name,mut token)=(String::new(),String::new(),String::new());
 for (k,v) in u.query_pairs(){match k.as_ref(){"p"=>id=v.into_owned(),"n"=>name=v.into_owned(),"t"|"token"=>token=v.into_owned(),_=>{}}}
 if id.is_empty(){return Err("Missing peer ID".into())} let alias=if !alias.trim().is_empty(){alias.trim().to_string()}else if !name.is_empty(){name}else{"Friend".into()};
 Ok(json!({"peerID":id,"alias":alias,"token":token,"allowUsingAsExitNode":false}))
}
struct App{tx:Sender<Job>,rx:Receiver<Event>,thread:Option<std::thread::JoinHandle<()>>,data:Snapshot,state:String,error:String,link:String,alias:String,invite:String,name:String,logs:Option<String>}
impl App{
 fn new(cc:&eframe::CreationContext<'_>)->Self{let (tx,jobs)=mpsc::channel();let (events,rx)=mpsc::channel();let ctx=cc.egui_ctx.clone();let thread=std::thread::spawn(move||worker(jobs,events,ctx));let _=tx.send(Job::Start);Self{tx,rx,thread:Some(thread),data:Snapshot::default(),state:"Starting…".into(),error:String::new(),link:String::new(),alias:String::new(),invite:String::new(),name:String::new(),logs:None}}
 fn job(&mut self,j:Job){self.error.clear();let _=self.tx.send(j);}
}
impl Drop for App{fn drop(&mut self){let _=self.tx.send(Job::Exit);if let Some(t)=self.thread.take(){let _=t.join();}}}
impl eframe::App for App{
 fn update(&mut self,ctx:&egui::Context,_:&mut eframe::Frame){
  while let Ok(e)=self.rx.try_recv(){match e{Event::State(s)=>self.state=s,Event::Data(d)=>self.data=d,Event::Error(e)=>self.error=e,Event::Invite(i)=>{ctx.copy_text(i.clone());self.invite=i;},Event::Logs(l)=>self.logs=Some(l)}}
  egui::CentralPanel::default().show(ctx,|ui|{
   ui.horizontal(|ui|{ui.heading("LiteLAN");ui.label("0.1 • AWL 0.21");});ui.add_space(8.);
   ui.horizontal(|ui|{if ui.button("Start").clicked(){self.job(Job::Start)}if ui.button("Stop").clicked(){self.job(Job::Stop)}if ui.button("Logs").clicked(){self.job(Job::Logs)}});
   ui.label(&self.state);if !self.error.is_empty(){ui.colored_label(Color32::LIGHT_RED,&self.error);}
   let ready=self.data.me.is_object();let ip=s(&self.data.me["vpn"],"ipnet").split('/').next().unwrap_or("").to_string();
   ui.separator();ui.horizontal(|ui|{ui.label("Your VPN IP");ui.label(RichText::new(if ip.is_empty(){"—"}else{&ip}).size(23.).strong());if !ip.is_empty()&&ui.button("Copy").clicked(){ctx.copy_text(ip.clone());}});
   if ready{ui.label(format!("{} • bootstrap {}/{}",s(&self.data.me,"name"),self.data.me["connectedBootstrapPeers"],self.data.me["totalBootstrapPeers"]));}
   ui.add_enabled_ui(ready,|ui|{
    ui.horizontal(|ui|{ui.text_edit_singleline(&mut self.name);if ui.button("Rename").clicked()&&!self.name.trim().is_empty(){self.job(Job::Rename(self.name.trim().into()));}});
    ui.separator();ui.horizontal(|ui|{ui.heading("Devices");ui.label(format!("{} online",arr(&self.data.peers).iter().filter(|p|yes(p,"connected")).count()));});
    egui::ScrollArea::vertical().max_height(190.).show(ui,|ui|{for p in arr(&self.data.peers){let live=yes(p,"connected");let con=arr(&p["connections"]);let route=if !live{"Offline"}else if con.iter().any(|c|!yes(c,"throughRelay")){"Direct available"}else if !con.is_empty(){"Relay"}else{"Connected"};ui.horizontal(|ui|{ui.colored_label(if live{Color32::LIGHT_GREEN}else{Color32::GRAY},"●");ui.label(s(p,"alias"));ui.label(s(p,"ipAddr"));ui.label(route);if !s(p,"ipAddr").is_empty()&&ui.small_button("Copy IP").clicked(){ctx.copy_text(s(p,"ipAddr").into());}});if !live {let e=s(&p["reconnect"],"lastError");if !e.is_empty(){ui.small(e);}}}});
    let requests=arr(&self.data.requests).to_vec();for r in requests{ui.horizontal(|ui|{let name=s(&r,"name");let id=s(&r,"peerID");ui.label(format!("Request: {}",if name.is_empty(){id}else{name}));if ui.button("Accept").clicked(){self.job(Job::Accept(id.into(),if name.is_empty(){"Friend".into()}else{name.into()},false));}if ui.button("Decline").clicked(){self.job(Job::Accept(id.into(),String::new(),true));}});}
    ui.separator();if ui.button("Create & copy one-time invitation").clicked(){self.job(Job::Invite)}
    if !self.invite.is_empty(){ui.label("Invitation copied. Send privately to your friend.");if ui.button("Copy again").clicked(){ctx.copy_text(self.invite.clone());}}
    ui.label("Add friend by invitation link");ui.add(egui::TextEdit::singleline(&mut self.link).hint_text("awl://invite?..."));ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut self.alias).hint_text("Optional alias"));if ui.button("Add").clicked(){self.job(Job::Add(self.link.clone(),self.alias.clone()));}});
   });
   ui.separator();ui.small("Local API only • No exit node • No browser • DNS and SOCKS disabled");
  });
  if let Some(log)=self.logs.as_mut(){let mut open=true;egui::Window::new("Core logs").open(&mut open).resizable(true).show(ctx,|ui|{if ui.button("Copy logs").clicked(){ctx.copy_text(log.clone());}egui::ScrollArea::vertical().max_height(450.).show(ui,|ui|{ui.add(egui::TextEdit::multiline(log).font(egui::TextStyle::Monospace).desired_width(600.));});});if !open{self.logs=None;}}
 }
}
fn main()->eframe::Result<()> {eframe::run_native("LiteLAN",eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_inner_size([520.,600.]).with_min_inner_size([460.,510.]),..Default::default()},Box::new(|cc|Ok(Box::new(App::new(cc)))))}
#[cfg(test)] mod tests{use super::*;#[test]fn parses_link(){let v=parse_invite("awl://invite?p=abc&n=My%20PC&t=secret","").unwrap();assert_eq!(v["peerID"],"abc");assert_eq!(v["alias"],"My PC");assert_eq!(v["token"],"secret");}#[test]fn rejects_http(){assert!(parse_invite("https://example.com","x").is_err());}}
