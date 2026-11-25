#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use eframe::egui::{self, Color32, RichText};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::{Child, Command, Stdio}, sync::mpsc::{self, Sender, Receiver}, time::{Duration, Instant}};
#[cfg(windows)] use std::os::windows::process::CommandExt;
const BASE: &str = "http://127.0.0.1:18639/api/v0";
type R<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String { e.to_string() }
fn normalize(v: Value) -> Value {
 match v { Value::Object(m) => Value::Object(m.into_iter().map(|(k,v)|(k.to_ascii_lowercase(),normalize(v))).collect()), Value::Array(a)=>Value::Array(a.into_iter().map(normalize).collect()), x=>x }
}
fn s<'a>(v: &'a Value, key: &str) -> &'a str { v[key.to_ascii_lowercase()].as_str().unwrap_or("") }
fn arr(v: &Value) -> &[Value] { v.as_array().map(Vec::as_slice).unwrap_or(&[]) }
fn yes(v: &Value, key: &str) -> bool { v[key.to_ascii_lowercase()].as_bool().unwrap_or(false) }
struct Api(reqwest::blocking::Client);
impl Api {
 fn new() -> R<Self> { Ok(Self(reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build().map_err(err)?)) }
 fn call(&self, path: &str, body: Option<Value>) -> R<String> {
  let u = format!("{BASE}{path}");
  let r = match body { Some(v) => self.0.post(u).json(&v), None => self.0.get(u) }.send().map_err(err)?;
  let status = r.status(); let text = r.text().map_err(err)?;
  if !status.is_success() { return Err(format!("HTTP {status}: {text}")); } Ok(text)
 }
 fn get(&self, p: &str) -> R<Value> { serde_json::from_str(&self.call(p,None)?).map(normalize).map_err(err) }
 fn post(&self,p:&str,v:Value)->R<String>{self.call(p,Some(v))}
}
fn profile() -> R<PathBuf> { Ok(PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA missing")?).join("LiteLAN").join("profile")) }
fn core_path()->R<PathBuf>{ Ok(std::env::current_exe().map_err(err)?.parent().ok_or("No executable directory")?.join("core").join("awl.exe")) }
fn default_name()->String { std::env::var("COMPUTERNAME").ok().filter(|s|!s.trim().is_empty()).unwrap_or_else(||"Windows PC".into()) }
fn start_core() -> R<Child> {
 let dir=profile()?; fs::create_dir_all(&dir).map_err(err)?;
 let file=dir.join("config_awl.json");
 let mut cfg=if file.exists(){serde_json::from_slice::<Value>(&fs::read(&file).map_err(err)?).map_err(err)?}else{json!({})};
 if !cfg.is_object(){return Err("Invalid profile JSON".into())}
 if file.exists(){fs::copy(&file,dir.join("config_awl.backup.json")).map_err(err)?;}
 for k in ["p2pNode","vpn","vpnGateway","socks5","dns","update","httpBasicAuth"] {if !cfg[k].is_object(){cfg[k]=json!({});}}
 cfg["httpListenAddress"]=json!("127.0.0.1:18639"); cfg["httpListenOnAdminHost"]=json!(false);
 cfg["httpBasicAuth"]=json!({"username":"","password":""});
 cfg["loggerLevel"]=json!("debug"); cfg["p2pNode"]["autoAcceptAuthRequests"]=json!(false);
 if cfg["p2pNode"]["name"].as_str().unwrap_or("").trim().is_empty(){cfg["p2pNode"]["name"]=json!(default_name());}
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
#[derive(Default)] struct Snapshot {me:Value, peers:Value, requests:Value,debug:Value}
enum Job {Start,Stop,Invite,Add(String,String),Accept(String,String,bool),Rename(String),Remove(String),Logs,Exit}
enum Event {Added(String), ActionDone, State(String),Data(Snapshot),Invite(String),Logs(String),Error(String)}
fn worker(rx:Receiver<Job>,tx:Sender<Event>,ctx:egui::Context){
 let send=|e|{let _=tx.send(e);ctx.request_repaint();};
 let api=match Api::new(){Ok(a)=>a,Err(e)=>{send(Event::Error(e));return}};
 let mut child:Option<Child>=None; let mut started=Instant::now(); let mut next=Instant::now();
 loop {
  match rx.recv_timeout(Duration::from_millis(200)) {
   Ok(Job::Exit)|Err(mpsc::RecvTimeoutError::Disconnected)=>{stop(&mut child);break},
   Ok(Job::Start)=>{if child.is_none(){
    if std::net::TcpStream::connect_timeout(&"127.0.0.1:18639".parse().unwrap(),Duration::from_millis(300)).is_ok(){send(Event::State("Port already in use".into()));send(Event::Error("Port 18639 already in use. Close the other LiteLAN instance.".into()));}
    else {match start_core(){Ok(c)=>{child=Some(c);started=Instant::now();next=Instant::now();send(Event::State("Starting core…".into()));},Err(e)=>{send(Event::State("Core failed to start".into()));send(Event::Error(e));}}}
   }},
   Ok(Job::Stop)=>{stop(&mut child);send(Event::State("Stopped".into()));send(Event::Data(Snapshot::default()));},
   Ok(Job::Logs)=>{let text=api.call("/debug/log?logs=100",None).or_else(|_|fs::read_to_string(profile()?.join("core.log")).map_err(err));match text{Ok(t)=>send(Event::Logs(t)),Err(e)=>send(Event::Error(e))}},
   Ok(job)=>{
    let res=match job {
     Job::Invite=>api.post("/peers/invites/create",json!({"maxUses":1,"expiresInSeconds":86400,"allowUsingAsExitNode":false})).and_then(|t|{let v=normalize(serde_json::from_str::<Value>(&t).map_err(err)?);let link=s(&v,"link");if link.is_empty(){return Err("Invite response missing link".into())}send(Event::Invite(link.into()));Ok(String::new())}),
     Job::Add(link,alias)=>parse_invite(&link,&alias).and_then(|v|{let id=v["peerID"].as_str().unwrap_or("").to_owned();api.get("/settings/peer_info").and_then(|me|{if s(&me,"peerid")==id{return Err("Cannot add your own device".into())}api.post("/peers/invite_peer",v)}).map(|t|{send(Event::Added(id));t})}),
     Job::Accept(id,alias,decline)=>api.post("/peers/accept_peer",json!({"peerID":id,"alias":alias,"decline":decline,"allowUsingAsExitNode":false})),
     Job::Rename(name)=>api.post("/settings/update",json!({"name":if name.trim().is_empty(){default_name()}else{name}})),
     Job::Remove(id)=>api.post("/peers/remove",json!({"peerID":id})),
     _=>Ok(String::new())};
    match res{Ok(_)=>{send(Event::ActionDone);next=Instant::now();},Err(e)=>send(Event::Error(e))}
   },Err(mpsc::RecvTimeoutError::Timeout)=>{}
  }
  if let Some(c)=child.as_mut(){match c.try_wait(){Ok(Some(status))=>{child=None;send(Event::State(format!("Core exited: {status}. Open logs.")));send(Event::Data(Snapshot::default()));continue},Err(e)=>{send(Event::Error(err(e)));},_=>{}}}
  if child.is_some() && Instant::now()>=next{
   next=Instant::now()+Duration::from_secs(3);
   match api.get("/settings/peer_info").and_then(|me|Ok(Snapshot{me,peers:api.get("/peers/get_known")?,requests:api.get("/peers/auth_requests")?,debug:api.get("/debug/p2p_info").unwrap_or(Value::Null)})){
    Ok(s)=>{let ready=network_ready(&s);send(Event::State(if ready {"Network ready"} else {"Discovering network"}.into()));send(Event::Data(s));},
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
struct App{tx:Sender<Job>,rx:Receiver<Event>,thread:Option<std::thread::JoinHandle<()>>,data:Snapshot,state:String,error:String,link:String,alias:String,invite:String,name:String,logs:Option<String>,page:usize,dark:bool,scale:f32,busy:bool,pending:Option<(String,Instant)>,notice:String,name_loaded:bool}
impl App{
 fn new(cc:&eframe::CreationContext<'_>)->Self{install_fonts(&cc.egui_ctx);let (tx,jobs)=mpsc::channel();let (events,rx)=mpsc::channel();let ctx=cc.egui_ctx.clone();let thread=std::thread::spawn(move||worker(jobs,events,ctx));let _=tx.send(Job::Start);Self{tx,rx,thread:Some(thread),data:Snapshot::default(),state:"Starting…".into(),error:String::new(),link:String::new(),alias:String::new(),invite:String::new(),name:String::new(),logs:None,page:0,dark:true,scale:1.0,busy:false,pending:None,notice:String::new(),name_loaded:false}}
 fn job(&mut self,j:Job){self.error.clear();if matches!(&j,Job::Add(..)|Job::Invite|Job::Accept(..)|Job::Rename(..)|Job::Remove(..)){self.busy=true;self.notice.clear();}if self.tx.send(j).is_err(){self.busy=false;self.error="Worker unavailable".into();}}
}
impl Drop for App{fn drop(&mut self){let _=self.tx.send(Job::Exit);if let Some(t)=self.thread.take(){let _=t.join();}}}
fn network_ready(d:&Snapshot)->bool {
 yes(&d.me["vpn"],"vpninterfaceenabled") && !s(&d.me["vpn"],"ipnet").is_empty() && d.me["connectedbootstrappeers"].as_u64().unwrap_or(0)>0
}
fn install_fonts(ctx:&egui::Context){
 let mut fonts=egui::FontDefinitions::default();
 if let Some(root)=std::env::var_os("WINDIR") {if let Ok(bytes)=fs::read(PathBuf::from(root).join("Fonts").join("segoeui.ttf")) {fonts.font_data.insert("system-ui".into(),egui::FontData::from_owned(bytes).into());fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0,"system-ui".into());}}
 ctx.set_fonts(fonts);
}
fn c(r:u8,g:u8,b:u8)->Color32{Color32::from_rgb(r,g,b)}
fn theme(ctx:&egui::Context,dark:bool){
 let mut style=(*ctx.style()).clone();let mut v=if dark{egui::Visuals::dark()}else{egui::Visuals::light()};
 let (bg,panel,ink,sub,stroke,hover)=if dark{(c(17,20,27),c(24,29,39),c(235,239,247),c(148,160,178),c(43,51,65),c(35,45,61))}else{(c(245,247,251),Color32::WHITE,c(24,34,49),c(94,108,130),c(221,227,237),c(232,239,250))};
 v.panel_fill=bg;v.window_fill=panel;v.override_text_color=None;v.widgets.noninteractive.fg_stroke.color=ink;v.widgets.noninteractive.bg_stroke.color=stroke;v.widgets.noninteractive.weak_bg_fill=panel;v.selection.bg_fill=c(44,92,169);v.selection.stroke.color=Color32::WHITE;
 for w in [&mut v.widgets.inactive,&mut v.widgets.hovered,&mut v.widgets.active]{w.corner_radius=egui::CornerRadius::same(8);w.bg_stroke=egui::Stroke::new(1.,stroke);w.fg_stroke.color=ink;w.bg_fill=panel;w.weak_bg_fill=panel;}
 v.widgets.hovered.bg_fill=hover;v.widgets.hovered.weak_bg_fill=hover;v.widgets.active.bg_fill=hover;v.widgets.active.weak_bg_fill=hover;v.widgets.noninteractive.bg_fill=panel;v.faint_bg_color=hover;v.extreme_bg_color=bg;let _ = sub;
 style.visuals=v;style.spacing.item_spacing=egui::vec2(12.,10.);style.spacing.button_padding=egui::vec2(16.,9.);style.spacing.interact_size=egui::vec2(42.,36.);
 for (t,z) in [(egui::TextStyle::Body,15.),(egui::TextStyle::Button,15.),(egui::TextStyle::Heading,26.),(egui::TextStyle::Small,12.)]{style.text_styles.insert(t,egui::FontId::proportional(z));}ctx.set_style(style);
}
fn surface(ui:&mut egui::Ui,f:impl FnOnce(&mut egui::Ui)) {egui::Frame::new().fill(ui.visuals().window_fill).stroke(egui::Stroke::new(1.,ui.visuals().widgets.noninteractive.bg_stroke.color)).corner_radius(12).inner_margin(18).show(ui,|ui|{ui.set_width(ui.available_width());f(ui);});}
fn primary(ui:&mut egui::Ui,label:&str)->bool{ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE).strong()).fill(c(48,105,204)).stroke(egui::Stroke::NONE).corner_radius(8)).clicked()}
fn muted(ui:&mut egui::Ui,t:impl Into<String>){ui.label(RichText::new(t.into()).size(13.).weak());}
fn dot(ui:&mut egui::Ui,active:bool,pulse:bool){let (r,_)=ui.allocate_exact_size(egui::vec2(22.,22.),egui::Sense::hover());let color=if active{c(39,192,129)}else{c(133,143,158)};if active&&pulse{let t=(ui.input(|i|i.time)*0.65).fract() as f32;ui.painter().circle_filled(r.center(),5.+7.*t,color.gamma_multiply((1.-t)*0.35));ui.ctx().request_repaint_after(Duration::from_millis(50));}ui.painter().circle_filled(r.center(),4.5,color);}
fn glyph_at(ui:&egui::Ui,r:egui::Rect,i:usize,color:Color32){let p=ui.painter();let st=egui::Stroke::new(1.6,color);let at=|x:f32,y:f32|r.min+egui::vec2(x,y);match i {0=>{for (x,y) in [(2.,2.),(11.,2.),(2.,11.),(11.,11.)]{p.rect_stroke(egui::Rect::from_min_size(at(x,y),egui::vec2(7.,7.)),2,st,egui::StrokeKind::Inside);}},1=>{p.circle_stroke(at(7.,6.),4.,st);p.line_segment([at(2.,17.),at(2.,13.)],st);p.line_segment([at(2.,13.),at(12.,13.)],st);p.line_segment([at(15.,5.),at(15.,13.)],st);p.line_segment([at(11.,9.),at(19.,9.)],st);},2=>{for y in [4.,10.,16.]{p.line_segment([at(2.,y),at(18.,y)],st);p.circle_filled(at(if y==10.{13.}else{7.},y),2.5,color);}},_=>{p.rect_stroke(egui::Rect::from_min_max(at(2.,3.),at(18.,17.)),3,st,egui::StrokeKind::Inside);p.line_segment([at(5.,7.),at(9.,10.)],st);p.line_segment([at(9.,10.),at(5.,13.)],st);p.line_segment([at(11.,13.),at(15.,13.)],st);}}}
fn num(v:&Value)->String{v.as_u64().map(|n|n.to_string()).unwrap_or_else(||"—".into())}
fn rate(v:&Value,k:&str)->String{let x=s(v,k);if x.is_empty(){"—".into()}else{x.into()}}
fn ping(p:&Value)->String{p["ping"].as_u64().filter(|n|*n>0).map(|n|format!("{:.0} ms",n as f64/1_000_000.)).unwrap_or_else(||"—".into())}
fn route(p:&Value)->&'static str{if yes(p,"declined"){return "Declined"}if !yes(p,"confirmed"){return if yes(p,"connected"){"Awaiting approval"}else{"Connecting / awaiting approval"}}if !yes(p,"connected"){return "Offline"}let con=arr(&p["connections"]);if con.iter().any(|c|c["throughrelay"].as_bool()==Some(false)&&!yes(c,"transient")){"Direct"}else if con.iter().any(|c|yes(c,"throughrelay")){"Relay"}else{"Connected"}}
impl eframe::App for App{
 fn update(&mut self,ctx:&egui::Context,_:&mut eframe::Frame){
  while let Ok(e)=self.rx.try_recv(){match e{Event::State(s)=>self.state=s,Event::Data(d)=>{if !self.name_loaded && d.me.is_object(){self.name=s(&d.me,"name").into();self.name_loaded=true;}self.data=d;},Event::Error(e)=>{self.busy=false;self.error=e;self.pending=None;},Event::ActionDone=>{self.busy=false;},Event::Added(id)=>{self.link.clear();self.alias.clear();self.pending=Some((id,Instant::now()));self.notice="Peer saved locally. Waiting for remote authorization and connection.".into();self.page=0;},Event::Invite(i)=>{ctx.copy_text(i.clone());self.invite=i;},Event::Logs(l)=>self.logs=Some(l)}}
  theme(ctx,self.dark);if (ctx.zoom_factor()-self.scale).abs()>0.01 {ctx.set_zoom_factor(self.scale);}
  let ready=network_ready(&self.data);let api=self.data.me.is_object();let ip=s(&self.data.me["vpn"],"ipnet").split('/').next().unwrap_or("").to_string();let online=arr(&self.data.peers).iter().filter(|p|yes(p,"connected")&&yes(p,"confirmed")&&!yes(p,"declined")).count();let total=arr(&self.data.peers).len();
  let boot=format!("{} / {}",num(&self.data.me["connectedbootstrappeers"]),num(&self.data.me["totalbootstrappeers"]));let dht=num(&self.data.debug["dht"]["routingtablesize"]);let stats=&self.data.me["networkstatsiniecunits"];let rx=rate(stats,"ratein");let tx=rate(stats,"rateout");
  let active=api||self.state.starts_with("Starting")||self.state.starts_with("API unavailable");
  egui::SidePanel::left("nav").resizable(false).exact_width(if ctx.screen_rect().width()<780. {160.}else{208.}).frame(egui::Frame::new().fill(ctx.style().visuals.window_fill).inner_margin(16)).show(ctx,|ui|{
   ui.add_space(10.);ui.horizontal(|ui|{let (r,_)=ui.allocate_exact_size(egui::vec2(34.,34.),egui::Sense::hover());ui.painter().rect_filled(r,9,c(48,105,204));for (x,y) in [(9.,9.),(25.,9.),(17.,25.)]{ui.painter().circle_filled(r.min+egui::vec2(x,y),3.,Color32::WHITE);}for (a,b) in [(egui::vec2(9.,9.),egui::vec2(17.,25.)),(egui::vec2(25.,9.),egui::vec2(17.,25.))]{ui.painter().line_segment([r.min+a,r.min+b],egui::Stroke::new(1.7,Color32::WHITE));}ui.label(RichText::new("LiteLAN").size(23.).strong());});muted(ui,"PRIVATE NETWORK");ui.add_space(24.);
   for (i,label) in ["Overview","Invitations","Settings","Diagnostics"].iter().enumerate(){let selected=self.page==i;let (r,response)=ui.allocate_exact_size(egui::vec2(ui.available_width(),44.),egui::Sense::click());let color=if selected{if self.dark{c(106,163,255)}else{c(34,86,174)}}else{ui.visuals().text_color()};if selected||response.hovered(){ui.painter().rect_filled(r,8,if self.dark{c(31,46,70)}else{c(221,234,255)});}let icon_rect=egui::Rect::from_min_size(r.left_center()+egui::vec2(12.,-10.),egui::vec2(20.,20.));glyph_at(ui,icon_rect,i,color);ui.painter().text(r.left_center()+egui::vec2(44.,0.),egui::Align2::LEFT_CENTER,*label,egui::FontId::proportional(15.),color);if response.clicked(){self.page=i;}}
   ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT),|ui|{muted(ui,"LiteLAN 0.1.4 · AWL 0.21");ui.checkbox(&mut self.dark,"Dark appearance");});
  });
  egui::TopBottomPanel::bottom("status").frame(egui::Frame::new().fill(ctx.style().visuals.window_fill).inner_margin(10)).show(ctx,|ui|{ui.horizontal(|ui|{dot(ui,ready,false);ui.label(if ready{"Network ready"}else{"Not ready"});ui.separator();muted(ui,format!("{online}/{total} peers online"));ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{muted(ui,"Local API · Private LAN");});});});
  egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(20)).show(ctx,|ui|{egui::ScrollArea::vertical().id_salt(self.page).show(ui,|ui|{
   if !self.error.is_empty(){if ui.small_button("Dismiss error").clicked(){self.error.clear();}ui.colored_label(c(229,105,105),&self.error);}
   if !self.notice.is_empty(){ui.horizontal_wrapped(|ui|{ui.label(&self.notice);if ui.small_button("Dismiss").clicked(){self.notice.clear();}});}
   if self.busy || self.pending.is_some(){ui.horizontal_wrapped(|ui|{ui.spinner();ui.label(if self.busy{"Working..."}else{"Waiting for peer approval..."});if let Some((_,since))=&self.pending{muted(ui,format!("{} s",since.elapsed().as_secs()));}if !self.busy && ui.small_button("Hide").clicked(){self.pending=None;}});ctx.request_repaint_after(Duration::from_millis(250));}
    for r in arr(&self.data.requests).to_vec(){surface(ui,|ui|{let id=s(&r,"peerid");let name=s(&r,"name");ui.label(RichText::new(format!("Incoming request: {}",if name.is_empty(){id}else{name})).strong());ui.add_enabled_ui(!self.busy,|ui|{ui.horizontal(|ui|{if primary(ui,"Accept"){self.job(Job::Accept(id.into(),if name.is_empty(){"Friend".into()}else{name.into()},false));}if ui.button("Decline").clicked(){self.job(Job::Accept(id.into(),String::new(),true));}});});});}
   match self.page {
    0=>{ui.horizontal(|ui|{ui.heading("Overview");ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{ui.add_enabled_ui(api,|ui|{if primary(ui,"+ Add peer"){self.page=1;}});});});muted(ui,"Manage your private multiplayer network");ui.add_space(10.);
     surface(ui,|ui|{ui.horizontal(|ui|{dot(ui,ready,true);ui.label(RichText::new(if ready{"Network ready"}else if active{"Connecting"}else{"Disconnected"}).size(19.).strong());ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{if active{if ui.button("Disconnect").clicked(){self.job(Job::Stop);}}else if primary(ui,"Connect"){self.job(Job::Start);}});});muted(ui,self.state.clone());ui.add_space(5.);ui.separator();ui.columns(3,|cols|{muted(&mut cols[0],"VIRTUAL IP");cols[0].label(RichText::new(if ip.is_empty(){"—"}else{&ip}).size(21.).strong());if !ip.is_empty()&&cols[0].small_button("Copy IP").clicked(){ctx.copy_text(ip.clone());}muted(&mut cols[1],"BOOTSTRAP NODES");cols[1].label(RichText::new(&boot).size(24.).strong());muted(&mut cols[2],"DHT ROUTING TABLE");cols[2].label(RichText::new(&dht).size(24.).strong());});});
     ui.add_space(8.);ui.horizontal(|ui|{ui.label(RichText::new("Devices").size(20.).strong());muted(ui,format!("{online} online / {total} total"));});
     surface(ui,|ui|{
       if total==0{ui.add_space(18.);ui.vertical_centered(|ui|{ui.label(RichText::new("Your network is empty").size(18.).strong());muted(ui,"Invite a friend to start playing together.");ui.add_enabled_ui(api && !self.busy,|ui|{if primary(ui,"Invite a friend"){self.page=1;}});});ui.add_space(18.);}
       else {for (index,p) in arr(&self.data.peers).to_vec().iter().enumerate(){
        if index>0{ui.separator();}
        let connected=yes(p,"connected")&&yes(p,"confirmed")&&!yes(p,"declined");
        ui.horizontal(|ui|{dot(ui,connected,false);ui.vertical(|ui|{let alias=s(p,"alias");ui.label(RichText::new(if alias.is_empty(){"Unnamed device"}else{alias}).strong());muted(ui,format!("{} / {}",route(p),ping(p)));});});
        ui.horizontal_wrapped(|ui|{
         let address=s(p,"ipaddr");ui.label(RichText::new(if address.is_empty(){"No virtual IP"}else{address}).monospace());
         if !address.is_empty()&&ui.small_button("Copy IP").clicked(){ctx.copy_text(address.into());}
         ui.add_enabled_ui(api&&!self.busy,|ui|{ui.menu_button("Actions",|ui|{if ui.button("Remove peer").clicked(){self.job(Job::Remove(s(p,"peerid").into()));ui.close_menu();}});});
        });
       }}
      });
      ui.add_space(6.);ui.columns(2,|cols|{surface(&mut cols[0],|ui|{muted(ui,"NETWORK DOWNLOAD");ui.label(RichText::new(&rx).size(22.).strong());});surface(&mut cols[1],|ui|{muted(ui,"NETWORK UPLOAD");ui.label(RichText::new(&tx).size(22.).strong());});});

    },
    1=>{ui.heading("Invitations");muted(ui,"Connect a friend to your network");ui.add_space(12.);ui.add_enabled_ui(api,|ui|{surface(ui,|ui|{ui.label(RichText::new("Create invitation").size(19.).strong());muted(ui,"Single-use link, valid for 24 hours.");if ui.add_enabled(!self.busy,egui::Button::new("Create & copy invitation")).clicked(){self.job(Job::Invite);}if ui.button("Copy device link (manual approval)").clicked(){let id=s(&self.data.me,"peerid");if !id.is_empty(){if let Ok(mut u)=url::Url::parse("awl://invite"){u.query_pairs_mut().append_pair("p",id).append_pair("n",s(&self.data.me,"name"));ctx.copy_text(u.to_string());self.notice="Device link copied. Accept the incoming request on this device.".into();}}}if !self.invite.is_empty(){ui.label("Invitation copied. Share it privately.");if ui.button("Copy again").clicked(){ctx.copy_text(self.invite.clone());}}});surface(ui,|ui|{ui.label(RichText::new("Join a friend").size(19.).strong());muted(ui,"Invitation link");ui.add(egui::TextEdit::singleline(&mut self.link).hint_text("awl://invite?…").desired_width(f32::INFINITY));muted(ui,"Device alias (optional)");ui.add(egui::TextEdit::singleline(&mut self.alias).desired_width(f32::INFINITY));if ui.add_enabled(!self.busy && !self.link.trim().is_empty(),egui::Button::new("Add peer")).clicked(){self.job(Job::Add(self.link.clone(),self.alias.clone()));}});});},
    2=>{ui.heading("Settings");muted(ui,"Customize your workspace and device identity.");ui.add_space(12.);surface(ui,|ui|{ui.label(RichText::new("Appearance").size(19.).strong());ui.checkbox(&mut self.dark,"Dark appearance");ui.add(egui::Slider::new(&mut self.scale,0.85..=1.5).step_by(0.05).text("UI scale"));if ui.button("Reset scale").clicked(){self.scale=1.;}});surface(ui,|ui|{ui.label(RichText::new("Device name").strong());ui.add_enabled_ui(api&&!self.busy,|ui|{ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY));if primary(ui,"Save"){if self.name.trim().is_empty(){self.name=default_name();}self.job(Job::Rename(self.name.trim().into()));}});muted(ui,"Private LAN only · No DNS, SOCKS5 or exit node");});},
    _=>{ui.heading("Diagnostics");muted(ui,"Inspect the network core and connection health.");ui.add_space(12.);surface(ui,|ui|{ui.label(&self.state);ui.label(format!("Bootstrap: {boot} · DHT: {dht}"));ui.label(format!("Adapter: {}",if yes(&self.data.me["vpn"],"vpninterfaceenabled"){"Enabled"}else{"Unavailable"}));ui.horizontal(|ui|{if ui.add_enabled(!active,egui::Button::new("Start")).clicked(){self.job(Job::Start);}if ui.add_enabled(active,egui::Button::new("Stop")).clicked(){self.job(Job::Stop);}if primary(ui,"View core logs"){self.job(Job::Logs);}});});}
   }
  });});
  if let Some((id,since))=self.pending.clone(){
   let peer=arr(&self.data.peers).iter().find(|p|s(p,"peerid")==id.as_str());
   let declined=peer.is_some_and(|p|yes(p,"declined"));
   let connected=peer.is_some_and(|p|yes(p,"confirmed")&&yes(p,"connected")&&!s(p,"ipaddr").is_empty());
   if connected||declined||since.elapsed()>Duration::from_secs(60){self.pending=None;self.notice=if connected{"Peer connected. You can now test the virtual IP."}else if declined{"The remote peer declined the request."}else{"Still waiting for remote peer. The core may retry in the background; inspect diagnostics on both PCs."}.into();}
  }
  if let Some(log)=self.logs.as_mut(){let mut open=true;egui::Window::new("Core logs").open(&mut open).resizable(true).show(ctx,|ui|{muted(ui,"Logs may contain private information. Review before sharing.");if ui.button("Copy logs").clicked(){ctx.copy_text(log.clone());}egui::ScrollArea::vertical().max_height(450.).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut log.as_str()).font(egui::TextStyle::Monospace).desired_width(650.));});});if !open{self.logs=None;}}
 }
}
fn main()->eframe::Result<()> {eframe::run_native("LiteLAN",eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_inner_size([1100.,750.]).with_min_inner_size([960.,650.]),..Default::default()},Box::new(|cc|Ok(Box::new(App::new(cc))))) }
#[cfg(test)] mod tests{use super::*;#[test]fn parses_link(){let v=parse_invite("awl://invite?p=abc&n=My%20PC&t=secret","").unwrap();assert_eq!(v["peerID"],"abc");assert_eq!(v["alias"],"My PC");assert_eq!(v["token"],"secret");}#[test]fn rejects_http(){assert!(parse_invite("https://example.com","x").is_err());}#[test]fn go_json(){let me=normalize(json!({"VPN":{"IPNet":"10.64.160.184/11","VPNInterfaceEnabled":true},"ConnectedBootstrapPeers":4}));assert!(network_ready(&Snapshot{me,..Default::default()}));assert_eq!(ping(&normalize(json!({"Ping":18000000}))),"18 ms");assert_eq!(route(&normalize(json!({"Connected":true,"Confirmed":true,"Connections":[{"ThroughRelay":false}]}))),"Direct");}}
