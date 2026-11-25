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
struct App{tx:Sender<Job>,rx:Receiver<Event>,thread:Option<std::thread::JoinHandle<()>>,data:Snapshot,state:String,error:String,link:String,alias:String,invite:String,name:String,logs:Option<String>,page:usize,dark:bool}
impl App{
 fn new(cc:&eframe::CreationContext<'_>)->Self{let (tx,jobs)=mpsc::channel();let (events,rx)=mpsc::channel();let ctx=cc.egui_ctx.clone();let thread=std::thread::spawn(move||worker(jobs,events,ctx));let _=tx.send(Job::Start);Self{tx,rx,thread:Some(thread),data:Snapshot::default(),state:"Starting…".into(),error:String::new(),link:String::new(),alias:String::new(),invite:String::new(),name:String::new(),logs:None,page:0,dark:false}}
 fn job(&mut self,j:Job){self.error.clear();let _=self.tx.send(j);}
}
impl Drop for App{fn drop(&mut self){let _=self.tx.send(Job::Exit);if let Some(t)=self.thread.take(){let _=t.join();}}}
fn theme(ctx:&egui::Context,dark:bool){
 let mut style=(*ctx.style()).clone();
 let mut v=if dark{egui::Visuals::dark()}else{egui::Visuals::light()};
 let (bg,panel,ink,accent,hover)=if dark{(Color32::from_rgb(23,25,33),Color32::from_rgb(32,35,45),Color32::from_rgb(225,229,240),Color32::from_rgb(62,79,122),Color32::from_rgb(48,54,70))}else{(Color32::from_rgb(248,247,252),Color32::from_rgb(239,240,248),Color32::from_rgb(32,36,52),Color32::from_rgb(216,225,250),Color32::from_rgb(229,232,246))};
 v.panel_fill=bg;v.window_fill=panel;v.override_text_color=Some(ink);v.selection.bg_fill=accent;
 v.widgets.inactive.bg_fill=panel;v.widgets.inactive.weak_bg_fill=panel;
 v.widgets.hovered.bg_fill=hover;v.widgets.hovered.weak_bg_fill=hover;
 v.widgets.active.bg_fill=accent;v.widgets.active.weak_bg_fill=accent;
 for w in [&mut v.widgets.inactive,&mut v.widgets.hovered,&mut v.widgets.active]{w.corner_radius=egui::CornerRadius::same(8);w.bg_stroke=egui::Stroke::NONE;}
 style.visuals=v;style.spacing.item_spacing=egui::vec2(10.,9.);style.spacing.button_padding=egui::vec2(12.,7.);
 style.text_styles.insert(egui::TextStyle::Body,egui::FontId::proportional(14.));style.text_styles.insert(egui::TextStyle::Button,egui::FontId::proportional(14.));style.text_styles.insert(egui::TextStyle::Heading,egui::FontId::proportional(23.));ctx.set_style(style);
}
fn surface(ui:&mut egui::Ui, f:impl FnOnce(&mut egui::Ui)){
 egui::Frame::new().fill(ui.visuals().widgets.inactive.bg_fill).corner_radius(12).inner_margin(16).show(ui,f);
}
fn primary(ui:&mut egui::Ui,label:&str)->bool{ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(Color32::from_rgb(65,92,175)).corner_radius(8)).clicked()}
fn muted(ui:&mut egui::Ui,t:impl Into<String>){ui.label(RichText::new(t.into()).size(12.).weak());}
impl eframe::App for App{
 fn update(&mut self,ctx:&egui::Context,_:&mut eframe::Frame){
  while let Ok(e)=self.rx.try_recv(){match e{Event::State(s)=>self.state=s,Event::Data(d)=>self.data=d,Event::Error(e)=>self.error=e,Event::Invite(i)=>{ctx.copy_text(i.clone());self.invite=i;},Event::Logs(l)=>self.logs=Some(l)}}
  theme(ctx,self.dark);
  let ready=self.data.me.is_object();let ip=s(&self.data.me["vpn"],"ipnet").split('/').next().unwrap_or("").to_string();
  let online=arr(&self.data.peers).iter().filter(|p|yes(p,"connected")).count();let total=arr(&self.data.peers).len();
  egui::SidePanel::left("navigation").resizable(false).exact_width(174.).frame(egui::Frame::new().fill(ctx.style().visuals.widgets.inactive.bg_fill).inner_margin(12)).show(ctx,|ui|{
   ui.add_space(8.);ui.label(RichText::new("LiteLAN").size(25.).strong());muted(ui,"Private mesh network");ui.add_space(22.);
   for (i,label) in ["Overview","Invitations","Settings","Diagnostics"].iter().enumerate(){if ui.add_sized([150.,38.],egui::Button::new(*label).selected(self.page==i)).clicked(){self.page=i;}}
   ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT),|ui|{muted(ui,"LiteLAN • AWL 0.21");ui.checkbox(&mut self.dark,"Dark appearance");});
  });
  egui::TopBottomPanel::bottom("status").show(ctx,|ui|{ui.horizontal(|ui|{ui.colored_label(if ready{Color32::from_rgb(38,155,93)}else{Color32::GRAY},"●");ui.label(if ready{"Core running"}else{"Core not ready"});ui.separator();muted(ui,format!("{online}/{total} peers online"));ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{muted(ui,"Local API · No exit node");});});});
  egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(22)).show(ctx,|ui|{
   egui::ScrollArea::vertical().show(ui,|ui|{
    if !self.error.is_empty(){ui.colored_label(Color32::from_rgb(210,76,76),&self.error);}
    match self.page{
     0=>{
      ui.horizontal(|ui|{ui.heading("Overview");ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{if ui.add_enabled(ready,egui::Button::new("+ Add peer")).clicked(){self.page=1;}});});
      muted(ui,"Your devices and private network");ui.add_space(10.);
      ui.columns(2,|cols|{
       cols[0].heading("Peers");muted(&mut cols[0],format!("{online} of {total} online"));cols[0].add_space(8.);
       if total==0{surface(&mut cols[0],|ui|{ui.set_min_height(120.);ui.label(RichText::new("No peers yet").strong());muted(ui,"Share an invitation to connect another computer.");if ui.add_enabled(ready,egui::Button::new("Add your first peer")).clicked(){self.page=1;}});}
       for p in arr(&self.data.peers){let live=yes(p,"connected");let con=arr(&p["connections"]);let route=if !live{"Offline"}else if con.iter().any(|c|!yes(c,"throughRelay")){"Direct available"}else if !con.is_empty(){"Relay"}else{"Connected"};surface(&mut cols[0],|ui|{ui.horizontal(|ui|{ui.colored_label(if live{Color32::from_rgb(38,155,93)}else{Color32::GRAY},"●");ui.label(RichText::new(s(p,"alias")).strong());});muted(ui,route);ui.separator();muted(ui,"VPN address");ui.horizontal(|ui|{ui.label(s(p,"ipAddr"));if !s(p,"ipAddr").is_empty()&&ui.small_button("Copy").clicked(){ctx.copy_text(s(p,"ipAddr").into());}});if !live{muted(ui,s(&p["reconnect"],"lastError"));}});cols[0].add_space(4.);}
       cols[1].heading("This device");muted(&mut cols[1],if ready{s(&self.data.me,"name")}else{"Not connected"});cols[1].add_space(8.);
       surface(&mut cols[1],|ui|{muted(ui,"Virtual IP address");ui.label(RichText::new(if ip.is_empty(){"—"}else{&ip}).size(25.).strong());if !ip.is_empty()&&ui.button("Copy address").clicked(){ctx.copy_text(ip.clone());}ui.separator();muted(ui,"Network status");ui.label(&self.state);if ready{muted(ui,format!("Bootstrap nodes: {} / {}",self.data.me["connectedBootstrapPeers"],self.data.me["totalBootstrapPeers"]));}ui.horizontal(|ui|{if ready{if ui.button("Disconnect").clicked(){self.job(Job::Stop);}}else if primary(ui,"Connect"){self.job(Job::Start);}});});
       cols[1].add_space(8.);surface(&mut cols[1],|ui|{ui.label(RichText::new("Network mode").strong());muted(ui,"Private LAN only");ui.label("Internet routing unchanged");});
      });
      let requests=arr(&self.data.requests).to_vec();if !requests.is_empty(){ui.add_space(12.);ui.heading("Incoming requests");for r in requests{surface(ui,|ui|{let name=s(&r,"name");let id=s(&r,"peerID");ui.label(if name.is_empty(){id}else{name});ui.horizontal(|ui|{if primary(ui,"Accept"){self.job(Job::Accept(id.into(),if name.is_empty(){"Friend".into()}else{name.into()},false));}if ui.button("Decline").clicked(){self.job(Job::Accept(id.into(),String::new(),true));}});});}}
     },
     1=>{ui.heading("Invitations");muted(ui,"Connect a friend to your private network");ui.add_space(12.);ui.add_enabled_ui(ready,|ui|{surface(ui,|ui|{ui.label(RichText::new("Invite a friend").size(18.).strong());muted(ui,"Single-use link, valid for 24 hours.");if primary(ui,"Create & copy invitation"){self.job(Job::Invite);}if !self.invite.is_empty(){ui.label("Invitation copied. Share it privately.");if ui.button("Copy again").clicked(){ctx.copy_text(self.invite.clone());}}});ui.add_space(10.);surface(ui,|ui|{ui.label(RichText::new("Join a friend").size(18.).strong());muted(ui,"Invitation link");ui.add(egui::TextEdit::singleline(&mut self.link).hint_text("awl://invite?...").desired_width(f32::INFINITY));muted(ui,"Device alias (optional)");ui.add(egui::TextEdit::singleline(&mut self.alias).desired_width(f32::INFINITY));if primary(ui,"Add peer"){self.job(Job::Add(self.link.clone(),self.alias.clone()));}});});if !ready{muted(ui,"Connect the network before creating or accepting invitations.");}},
     2=>{ui.heading("Settings");ui.add_space(12.);surface(ui,|ui|{ui.label(RichText::new("Device name").strong());ui.add_enabled_ui(ready,|ui|{ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("New device name"));if primary(ui,"Save")&&!self.name.trim().is_empty(){self.job(Job::Rename(self.name.trim().into()));}});});ui.separator();ui.checkbox(&mut self.dark,"Dark appearance");});ui.add_space(10.);surface(ui,|ui|{ui.label(RichText::new("Network configuration").strong());for t in ["Virtual adapter: litelan0","Local API: 127.0.0.1:18639","SOCKS5: disabled","DNS: disabled","Exit node: disabled"]{ui.label(t);}});},
     _=>{ui.heading("Diagnostics");muted(ui,"Inspect the network core and connection state");ui.add_space(12.);surface(ui,|ui|{ui.label(&self.state);ui.horizontal(|ui|{if ui.button("Start").clicked(){self.job(Job::Start);}if ui.button("Stop").clicked(){self.job(Job::Stop);}if primary(ui,"View core logs"){self.job(Job::Logs);}});});}
    }
   });
  });
  if let Some(log)=self.logs.as_mut(){let mut open=true;egui::Window::new("Core logs").open(&mut open).resizable(true).show(ctx,|ui|{if ui.button("Copy logs").clicked(){ctx.copy_text(log.clone());}egui::ScrollArea::vertical().max_height(450.).show(ui,|ui|{ui.add(egui::TextEdit::multiline(log).font(egui::TextStyle::Monospace).desired_width(600.));});});if !open{self.logs=None;}}
 }
}
fn main()->eframe::Result<()> {eframe::run_native("LiteLAN",eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_inner_size([940.,640.]).with_min_inner_size([780.,520.]),..Default::default()},Box::new(|cc|Ok(Box::new(App::new(cc))))) }
#[cfg(test)] mod tests{use super::*;#[test]fn parses_link(){let v=parse_invite("awl://invite?p=abc&n=My%20PC&t=secret","").unwrap();assert_eq!(v["peerID"],"abc");assert_eq!(v["alias"],"My PC");assert_eq!(v["token"],"secret");}#[test]fn rejects_http(){assert!(parse_invite("https://example.com","x").is_err());}}
