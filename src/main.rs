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
fn s_value(v:&Value,k:&str)->String{s(v,k).to_owned()}
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
 fn post_json(&self,p:&str,v:Value)->R<Value>{serde_json::from_str(&self.post(p,v)?).map(normalize).map_err(err)}
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
#[derive(Default)] struct Snapshot {me:Value, peers:Value, requests:Value,blocked:Value,debug:Value}
enum Job {Start,Stop,Invite,Add(String,String),Accept(String,String,bool),Rename(String),Retry(String),Cancel(String),Remove(String),Block(String),Unblock(String),Logs,Exit}
enum Event {Added(String),RetryQueued(String),Cancelled(String), ActionDone, State(String),Data(Snapshot),Invite(String),Logs(String),Error(String)}
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
     Job::Add(link,alias)=>parse_invite(&link,&alias).and_then(|v|{let id=v["peerID"].as_str().unwrap_or("").to_owned();api.get("/settings/peer_info").and_then(|me|{if s(&me,"peerid")==id{return Err("Cannot add your own device".into())}api.post("/peers/invite_peer",v).map_err(|e|if e.contains("peer has already been added"){"Device already saved. Open Overview to see its connection status; remove it first if you need to use a new invitation.".into()}else{e})}).map(|t|{send(Event::Added(id));t})}),
     Job::Accept(id,alias,decline)=>api.post("/peers/accept_peer",json!({"peerID":id,"alias":alias,"decline":decline,"allowUsingAsExitNode":false})),
     Job::Rename(name)=>api.post("/settings/update",json!({"name":if name.trim().is_empty(){default_name()}else{name}})),
     Job::Retry(id)=>api.post("/peers/litelan_retry",json!({"peerID":id})).map(|t|{send(Event::RetryQueued(id));t}),
     Job::Cancel(id)=>api.post("/peers/litelan_cancel",json!({"peerID":id})).map(|t|{send(Event::Cancelled(id));t}),
     Job::Remove(id)=>api.post("/peers/remove",json!({"peerID":id})).and_then(|_|api.post("/peers/unblock",json!({"peerID":id})).map_err(|e|format!("Device removed; unblock failed: {e}"))),
     Job::Block(id)=>api.post("/peers/remove",json!({"peerID":id})),
     Job::Unblock(id)=>api.post("/peers/unblock",json!({"peerID":id})),
     _=>Ok(String::new())};
    match res{Ok(_)=>{send(Event::ActionDone);next=Instant::now();},Err(e)=>send(Event::Error(e))}
   },Err(mpsc::RecvTimeoutError::Timeout)=>{}
  }
  if let Some(c)=child.as_mut(){match c.try_wait(){Ok(Some(status))=>{child=None;send(Event::State(format!("Core exited: {status}. Open logs.")));send(Event::Data(Snapshot::default()));continue},Err(e)=>{send(Event::Error(err(e)));},_=>{}}}
  if child.is_some() && Instant::now()>=next{
   next=Instant::now()+Duration::from_secs(3);
   match api.get("/settings/peer_info").and_then(|me|Ok(Snapshot{me,peers:api.get("/peers/get_known")?,requests:api.get("/peers/auth_requests")?,blocked:api.get("/peers/get_blocked").unwrap_or(Value::Null),debug:api.get("/debug/p2p_info").unwrap_or(Value::Null)})){
    Ok(mut s)=>{
      // AWL's known-peer list returns DisplayName in Name; fetch the actual remote name.
      if let Some(peers)=s.peers.as_array_mut(){for p in peers.iter_mut().filter(|p|yes(p,"confirmed")){
       let id=s_value(p,"peerid");
       if let Ok(settings)=api.post_json("/peers/get_known_peer_settings",json!({"peerID":id})){p["remotename"]=json!(s_value(&settings,"name"));}
      }}
      let ready=network_ready(&s);send(Event::State(if ready {"Network ready"} else {"Discovering network"}.into()));send(Event::Data(s));},
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
struct App{tx:Sender<Job>,rx:Receiver<Event>,thread:Option<std::thread::JoinHandle<()>>,data:Snapshot,state:String,error:String,link:String,alias:String,invite:String,name:String,logs:Option<String>,page:usize,dark:bool,scale:f32,busy:bool,pending:Option<(String,Instant)>,pending_hidden:bool,notice:String,notice_at:Instant,notice_kind:u8,name_loaded:bool,action_peer:Option<String>}
impl App{
 fn new(cc:&eframe::CreationContext<'_>)->Self{install_fonts(&cc.egui_ctx);let (tx,jobs)=mpsc::channel();let (events,rx)=mpsc::channel();let ctx=cc.egui_ctx.clone();let thread=std::thread::spawn(move||worker(jobs,events,ctx));let _=tx.send(Job::Start);Self{tx,rx,thread:Some(thread),data:Snapshot::default(),state:"Starting…".into(),error:String::new(),link:String::new(),alias:String::new(),invite:String::new(),name:String::new(),logs:None,page:0,dark:true,scale:1.0,busy:false,pending:None,pending_hidden:false,notice:String::new(),notice_at:Instant::now(),notice_kind:0,name_loaded:false,action_peer:None}}
 fn notify(&mut self,t:&str){self.notice=t.into();self.notice_at=Instant::now();self.notice_kind=if t.starts_with("Device connected")||t.contains("copied"){0}else{1};}
 fn job(&mut self,j:Job){self.error.clear();if matches!(&j,Job::Add(..)|Job::Invite|Job::Accept(..)|Job::Rename(..)|Job::Retry(..)|Job::Cancel(..)|Job::Remove(..)|Job::Block(..)|Job::Unblock(..)){self.busy=true;self.notice.clear();}if self.tx.send(j).is_err(){self.busy=false;self.error="Worker unavailable".into();}}
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
 style.visuals=v;style.spacing.item_spacing=egui::vec2(8.,8.);style.spacing.button_padding=egui::vec2(10.,6.);style.spacing.interact_size=egui::vec2(34.,36.);
 for (t,z) in [(egui::TextStyle::Body,14.),(egui::TextStyle::Button,13.),(egui::TextStyle::Heading,25.),(egui::TextStyle::Small,12.)]{style.text_styles.insert(t,egui::FontId::proportional(z));}ctx.set_style(style);
}
fn surface(ui:&mut egui::Ui,f:impl FnOnce(&mut egui::Ui)) {egui::Frame::new().fill(ui.visuals().window_fill).stroke(egui::Stroke::new(1.,ui.visuals().widgets.noninteractive.bg_stroke.color)).corner_radius(12).inner_margin(15).show(ui,|ui|{ui.set_width(ui.available_width());f(ui);});}
fn primary(ui:&mut egui::Ui,label:&str)->bool{ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE).strong()).fill(c(48,105,204)).stroke(egui::Stroke::NONE).corner_radius(8).min_size(egui::vec2(0.,36.))).clicked()}
fn muted(ui:&mut egui::Ui,t:impl Into<String>){ui.label(RichText::new(t.into()).size(13.).weak());}
fn dot(ui:&mut egui::Ui,active:bool,pulse:bool){let (r,_)=ui.allocate_exact_size(egui::vec2(22.,22.),egui::Sense::hover());let color=if active{c(39,192,129)}else{c(133,143,158)};if active&&pulse{let t=(ui.input(|i|i.time)*0.65).fract() as f32;ui.painter().circle_filled(r.center(),5.+7.*t,color.gamma_multiply((1.-t)*0.35));ui.ctx().request_repaint_after(Duration::from_millis(50));}ui.painter().circle_filled(r.center(),4.5,color);}
fn glyph_at(ui:&egui::Ui,r:egui::Rect,i:usize,color:Color32){let p=ui.painter();let st=egui::Stroke::new(1.6,color);let at=|x:f32,y:f32|r.min+egui::vec2(x,y);match i {0=>{for (x,y) in [(2.,2.),(11.,2.),(2.,11.),(11.,11.)]{p.rect_stroke(egui::Rect::from_min_size(at(x,y),egui::vec2(7.,7.)),2,st,egui::StrokeKind::Inside);}},1=>{p.circle_stroke(at(7.,6.),4.,st);p.line_segment([at(2.,17.),at(2.,13.)],st);p.line_segment([at(2.,13.),at(12.,13.)],st);p.line_segment([at(15.,5.),at(15.,13.)],st);p.line_segment([at(11.,9.),at(19.,9.)],st);},2=>{for y in [4.,10.,16.]{p.line_segment([at(2.,y),at(18.,y)],st);p.circle_filled(at(if y==10.{13.}else{7.},y),2.5,color);}},_=>{p.rect_stroke(egui::Rect::from_min_max(at(2.,3.),at(18.,17.)),3,st,egui::StrokeKind::Inside);p.line_segment([at(5.,7.),at(9.,10.)],st);p.line_segment([at(9.,10.),at(5.,13.)],st);p.line_segment([at(11.,13.),at(15.,13.)],st);}}}
fn num(v:&Value)->String{v.as_u64().map(|n|n.to_string()).unwrap_or_else(||"—".into())}
fn rate(v:&Value,k:&str)->String{let x=s(v,k);if x.is_empty(){"—".into()}else{x.into()}}
fn ping(p:&Value)->String{p["ping"].as_u64().filter(|n|*n>0).map(|n|format!("{:.0} ms",n as f64/1_000_000.)).unwrap_or_else(||"—".into())}
fn route(p:&Value)->&'static str{if yes(p,"declined"){return "Declined"}if !yes(p,"confirmed"){return if yes(p,"connected"){"Awaiting approval"}else{"Finding device"}}if !yes(p,"connected"){return "Offline"}if s(p,"ipaddr").is_empty(){return "Negotiating IP"}let con=arr(&p["connections"]);if con.iter().any(|c|c["throughrelay"].as_bool()==Some(false)&&!yes(c,"transient")){"Direct"}else if con.iter().any(|c|yes(c,"throughrelay")){"Relay"}else{"Connected"}}
fn peer_name(p:&Value)->String{let remote=s(p,"remotename");let alias=s(p,"alias");if !remote.trim().is_empty(){remote.into()}else if !alias.trim().is_empty(){alias.into()}else{let id=s(p,"peerid");format!("Device {}",id.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>())}}
fn status_color(p:&Value,dark:bool)->Color32{if yes(p,"declined"){c(229,105,105)}else if peer_ready(p){if route(p)=="Relay"{if dark{c(116,164,255)}else{c(34,86,174)}}else if dark{c(52,211,153)}else{c(16,128,83)}}else if !yes(p,"confirmed"){if dark{c(244,186,92)}else{c(162,99,16)}}else if dark{c(148,160,178)}else{c(94,108,130)}}
impl eframe::App for App{
 fn update(&mut self,ctx:&egui::Context,_:&mut eframe::Frame){
  while let Ok(e)=self.rx.try_recv(){match e{Event::State(s)=>{if s=="Stopped"||s.starts_with("Core exited"){self.pending=None;}self.state=s;},Event::Data(d)=>{if !self.name_loaded && d.me.is_object(){self.name=s(&d.me,"name").into();if self.name.trim().is_empty(){self.name=default_name();}self.name_loaded=true;}let new_connected=arr(&d.peers).iter().any(|p|yes(p,"connected")&&yes(p,"confirmed")&&!s(p,"ipaddr").is_empty()&&!arr(&self.data.peers).iter().any(|old|s(old,"peerid")==s(p,"peerid")&&yes(old,"connected")&&yes(old,"confirmed")&&!s(old,"ipaddr").is_empty()));let disconnected=d.me.is_object()&&self.data.me.is_object()&&arr(&self.data.peers).iter().any(|old|peer_ready(old)&&arr(&d.peers).iter().any(|p|s(p,"peerid")==s(old,"peerid")&&!peer_ready(p)));if new_connected{self.notify("Device connected. Virtual IP assigned.");}else if disconnected{self.notify("Device disconnected. Reconnecting automatically.");self.notice_kind=2;}self.data=d;},Event::Error(e)=>{self.busy=false;self.error=e;},Event::ActionDone=>{self.busy=false;},Event::RetryQueued(id)=>{self.pending=Some((id,Instant::now()));self.pending_hidden=false;self.notify("Retry requested");},Event::Cancelled(id)=>{if self.pending.as_ref().is_some_and(|(p,_)|p==&id){self.pending=None;}self.notify("Connection attempts cancelled");},Event::Added(id)=>{self.link.clear();self.alias.clear();self.pending=Some((id,Instant::now()));self.pending_hidden=false;self.notify("Invitation saved. Finding device…");self.page=0;},Event::Invite(i)=>{ctx.copy_text(i.clone());self.invite=i;self.notify("Invitation copied. Share privately.");},Event::Logs(l)=>self.logs=Some(l)}}
  theme(ctx,self.dark);if (ctx.zoom_factor()-self.scale).abs()>0.01 {ctx.set_zoom_factor(self.scale);}
  let ready=network_ready(&self.data);let api=self.data.me.is_object();let ip=s(&self.data.me["vpn"],"ipnet").split('/').next().unwrap_or("").to_string();let online=arr(&self.data.peers).iter().filter(|p|yes(p,"connected")&&yes(p,"confirmed")&&!yes(p,"declined")&&!s(p,"ipaddr").is_empty()).count();let total=arr(&self.data.peers).len();
  let boot=format!("{} / {}",num(&self.data.me["connectedbootstrappeers"]),num(&self.data.me["totalbootstrappeers"]));let dht=num(&self.data.debug["dht"]["routingtablesize"]);let stats=&self.data.me["networkstatsiniecunits"];let rx=rate(stats,"ratein");let tx=rate(stats,"rateout");
  let active=api||self.state.starts_with("Starting")||self.state.starts_with("API unavailable");
  egui::TopBottomPanel::top("header").frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(18,12))).show(ctx,|ui|{
   ui.horizontal(|ui|{let (r,_)=ui.allocate_exact_size(egui::vec2(34.,34.),egui::Sense::hover());brand_icon(ui,r);ui.label(RichText::new("LiteLAN").size(21.).strong());ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{muted(ui,"v0.1.7");});});
  });
  egui::TopBottomPanel::bottom("nav").frame(egui::Frame::new().fill(ctx.style().visuals.window_fill).inner_margin(egui::Margin::symmetric(8,8))).show(ctx,|ui|{
   let width=(ui.available_width()-3.*4.)/4.;ui.spacing_mut().item_spacing.x=4.;ui.horizontal(|ui|{
    for (i,label) in ["Overview","Connect","Settings","Diagnostics"].iter().enumerate(){let selected=self.page==i;let (r,response)=ui.allocate_exact_size(egui::vec2(width,57.),egui::Sense::click());let color=if selected{if self.dark{c(116,164,255)}else{c(34,86,174)}}else{ui.visuals().weak_text_color()};if selected||response.hovered(){ui.painter().rect_filled(r,9,if self.dark{c(31,46,70)}else{c(221,234,255)});}glyph_at(ui,egui::Rect::from_center_size(r.center_top()+egui::vec2(0.,19.),egui::vec2(20.,20.)),i,color);ui.painter().text(r.center_bottom()-egui::vec2(0.,12.),egui::Align2::CENTER_CENTER,*label,egui::FontId::proportional(11.),color);if response.clicked(){self.page=i;}}
   });
  });
  egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(19)).show(ctx,|ui|{egui::ScrollArea::vertical().id_salt(self.page).auto_shrink([false,false]).show(ui,|ui|{
   if self.busy {ui.horizontal(|ui|{ui.spinner();muted(ui,"Working…");});}
   if self.pending.is_some()&&self.pending_hidden {ui.horizontal_wrapped(|ui|{ui.spinner();muted(ui,"Connecting in background");if ui.button("Show progress").clicked(){self.pending_hidden=false;}});}
   match self.page {
    0=>{
     ui.horizontal(|ui|{ui.heading("Overview");ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{ui.add_enabled_ui(api,|ui|{if primary(ui,"+ Add peer"){self.page=1;}});});});muted(ui,"Your private network");ui.add_space(8.);
     surface(ui,|ui|{ui.horizontal(|ui|{dot(ui,ready,true);ui.label(RichText::new(if ready{"Network ready"}else if active{"Connecting"}else{"Disconnected"}).size(15.).strong());ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{if active{if ui.button("Disconnect").clicked(){self.job(Job::Stop);}}else if primary(ui,"Connect"){self.job(Job::Start);}});});
      if !ready{muted(ui,self.state.clone());}ui.add_space(8.);muted(ui,"YOUR VIRTUAL IP");ui.horizontal(|ui|{ui.label(RichText::new(if ip.is_empty(){"—"}else{&ip}).size(24.).strong());ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{if ui.add_enabled(!ip.is_empty(),egui::Button::new("Copy IP").min_size(egui::vec2(0.,36.))).clicked(){ctx.copy_text(ip.clone());self.notify("IP copied");}});});ui.separator();ui.columns(2,|cols|{muted(&mut cols[0],"Bootstrap nodes");cols[0].label(RichText::new(&boot).size(17.).strong());muted(&mut cols[1],"DHT routing table");cols[1].label(RichText::new(&dht).size(17.).strong());});});
     ui.add_space(8.);ui.horizontal(|ui|{ui.label(RichText::new("Devices").size(18.).strong());ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{muted(ui,format!("{online} online / {total}"));});});ui.separator();
     if total==0{ui.vertical_centered(|ui|{ui.add_space(18.);ui.label(RichText::new("Your network is empty").strong());muted(ui,"Invite a friend to start playing together.");ui.add_enabled_ui(api&&!self.busy,|ui|{if primary(ui,"Invite a friend"){self.page=1;}});ui.add_space(18.);});}
     for p in arr(&self.data.peers).to_vec(){
      let id=s(&p,"peerid").to_owned();let address=s(&p,"ipaddr");let connected=peer_ready(&p);let status=route(&p);let color=status_color(&p,self.dark);
      ui.push_id(&id,|ui|{let width=ui.available_width();let (r,_)=ui.allocate_exact_size(egui::vec2(width,66.),egui::Sense::hover());let icon=egui::Rect::from_center_size(r.left_center()+egui::vec2(19.,0.),egui::vec2(38.,38.));ui.painter().rect_filled(icon,10,ui.visuals().window_fill);let st=egui::Stroke::new(1.5,ui.visuals().weak_text_color());let center=icon.center();ui.painter().rect_stroke(egui::Rect::from_center_size(center-egui::vec2(0.,3.),egui::vec2(18.,12.)),2,st,egui::StrokeKind::Inside);ui.painter().line_segment([center+egui::vec2(0.,3.),center+egui::vec2(0.,8.)],st);ui.painter().line_segment([center+egui::vec2(-5.,8.),center+egui::vec2(5.,8.)],st);ui.painter().circle_filled(icon.right_bottom()-egui::vec2(3.,3.),5.,ui.visuals().panel_fill);ui.painter().circle_filled(icon.right_bottom()-egui::vec2(3.,3.),3.5,color);
       let menu_rect=egui::Rect::from_center_size(r.right_center()-egui::vec2(17.,0.),egui::vec2(34.,36.));let status_right=menu_rect.left()-8.;let name_left=r.left()+48.;let name_right=(status_right-112.).max(name_left+40.);let text_clip=egui::Rect::from_min_max(egui::pos2(name_left,r.top()),egui::pos2(name_right,r.bottom()));let painter=ui.painter().with_clip_rect(text_clip);painter.text(egui::pos2(name_left,r.center().y-10.),egui::Align2::LEFT_CENTER,peer_name(&p),egui::FontId::proportional(15.),ui.visuals().text_color());painter.text(egui::pos2(name_left,r.center().y+11.),egui::Align2::LEFT_CENTER,if address.is_empty(){"No IP yet"}else{address},egui::FontId::proportional(12.),ui.visuals().weak_text_color());ui.painter().text(egui::pos2(status_right,r.center().y-10.),egui::Align2::RIGHT_CENTER,status,egui::FontId::proportional(12.),color);ui.painter().text(egui::pos2(status_right,r.center().y+11.),egui::Align2::RIGHT_CENTER,if connected{ping(&p)}else{"—".into()},egui::FontId::proportional(12.),ui.visuals().weak_text_color());
       let clicked=ui.put(menu_rect,egui::Button::new("⋯").min_size(egui::vec2(34.,36.))).clicked();if clicked{self.action_peer=Some(id.clone());}
      });ui.separator();
     }
     ui.add_space(8.);ui.horizontal(|ui|{muted(ui,format!("↓ {rx}"));ui.add_space(12.);muted(ui,format!("↑ {tx}"));});
    },
    1=>{ui.heading("Connect");muted(ui,"Connect a friend to your network");ui.add_space(12.);ui.add_enabled_ui(api,|ui|{surface(ui,|ui|{ui.label(RichText::new("Invite a friend").size(19.).strong());muted(ui,"Recommended: single-use link, valid for 24 hours. No manual approval required for a valid token.");if ui.add_enabled(!self.busy,egui::Button::new(RichText::new("Create invitation").color(Color32::WHITE)).fill(c(52,107,218)).min_size(egui::vec2(ui.available_width(),42.))).clicked(){self.job(Job::Invite);}if ui.button("Copy device link (manual approval)").clicked(){let id=s(&self.data.me,"peerid");if !id.is_empty(){if let Ok(mut u)=url::Url::parse("awl://invite"){u.query_pairs_mut().append_pair("p",id).append_pair("n",s(&self.data.me,"name"));ctx.copy_text(u.to_string());self.notify("Device link copied. Accept the incoming request on this device.");}}}if !self.invite.is_empty(){ui.label("Invitation copied. Share it privately.");if ui.button("Copy again").clicked(){ctx.copy_text(self.invite.clone());self.notify("Invitation copied");}}});surface(ui,|ui|{ui.label(RichText::new("Join a friend").size(19.).strong());muted(ui,"Invitation link");ui.add(egui::TextEdit::singleline(&mut self.link).hint_text("awl://invite?…").desired_width(f32::INFINITY));muted(ui,"Device alias (optional)");ui.add(egui::TextEdit::singleline(&mut self.alias).desired_width(f32::INFINITY));if ui.add_enabled(!self.busy && !self.link.trim().is_empty(),egui::Button::new(RichText::new("Connect").color(Color32::WHITE)).fill(c(52,107,218)).min_size(egui::vec2(ui.available_width(),42.))).clicked(){self.job(Job::Add(self.link.clone(),self.alias.clone()));}});
      surface(ui,|ui|{ui.label(RichText::new("Blocked devices").strong());if self.data.blocked.is_null(){muted(ui,"Blocked list unavailable");}else if arr(&self.data.blocked).is_empty(){muted(ui,"No blocked devices");}for p in arr(&self.data.blocked).to_vec(){let id=s(&p,"peerid");let name=s(&p,"displayname");ui.horizontal_wrapped(|ui|{ui.label(if name.is_empty(){id}else{name});if !id.is_empty()&&ui.add_enabled(!self.busy,egui::Button::new("Unblock")).clicked(){self.job(Job::Unblock(id.into()));}});}});});},
    2=>{ui.heading("Settings");muted(ui,"Customize your workspace and device identity.");ui.add_space(12.);surface(ui,|ui|{ui.label(RichText::new("Appearance").size(19.).strong());ui.checkbox(&mut self.dark,"Dark appearance");ui.add(egui::Slider::new(&mut self.scale,0.85..=1.2).step_by(0.05).text("UI scale"));if ui.button("Reset scale").clicked(){self.scale=1.;}});surface(ui,|ui|{ui.label(RichText::new("Device name").strong());ui.add_enabled_ui(api&&!self.busy,|ui|{ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(f32::INFINITY));if primary(ui,"Save"){if self.name.trim().is_empty(){self.name=default_name();}self.job(Job::Rename(self.name.trim().into()));}});muted(ui,"Private LAN only · No DNS, SOCKS5 or exit node");});},
    _=>{ui.heading("Diagnostics");muted(ui,"Inspect the network core and connection health.");ui.add_space(12.);surface(ui,|ui|{ui.label(&self.state);ui.label(format!("Bootstrap: {boot} · DHT: {dht}"));muted(ui,format!("Reachability: {}",if s(&self.data.debug["dht"],"reachability").is_empty(){"Unknown"}else{s(&self.data.debug["dht"],"reachability")}));ui.label(format!("Adapter: {}",if yes(&self.data.me["vpn"],"vpninterfaceenabled"){"Enabled"}else{"Unavailable"}));ui.horizontal_wrapped(|ui|{if ui.add_enabled(!active,egui::Button::new("Start")).clicked(){self.job(Job::Start);}if ui.add_enabled(active,egui::Button::new("Stop")).clicked(){self.job(Job::Stop);}if primary(ui,"View core logs"){self.job(Job::Logs);}});});}
   }
  });});
  if self.notice_at.elapsed()>Duration::from_secs(5){self.notice.clear();}
  if !self.notice.is_empty()||!self.error.is_empty(){let is_error=!self.error.is_empty();let text=if is_error{self.error.clone()}else{self.notice.clone()};let toast_color=if is_error{c(229,105,105)}else{match self.notice_kind{1=>c(116,164,255),2=>c(211,157,68),_=>c(39,192,129)}};egui::Area::new(egui::Id::new("toast")).anchor(egui::Align2::CENTER_BOTTOM,egui::vec2(0.,-88.)).order(egui::Order::Foreground).show(ctx,|ui|{ui.set_width((ctx.screen_rect().width()-36.).min(480.));egui::Frame::new().fill(ui.visuals().window_fill).stroke(egui::Stroke::new(1.,toast_color)).corner_radius(10).inner_margin(12).show(ui,|ui|{ui.horizontal(|ui|{ui.colored_label(toast_color,if is_error||self.notice_kind==2{"!"}else if self.notice_kind==1{"i"}else{"✓"});ui.vertical(|ui|{ui.label(RichText::new(if is_error{"Something went wrong"}else{"LiteLAN"}).strong());ui.label(egui::RichText::new(&text).size(12.));});});if ui.small_button("Dismiss").clicked(){self.notice.clear();self.error.clear();}});});ctx.request_repaint_after(Duration::from_millis(250));}
  if let Some(id)=self.action_peer.clone(){if let Some(p)=arr(&self.data.peers).iter().find(|p|s(p,"peerid")==id.as_str()).cloned(){let mut open=true;egui::Window::new("Device actions").id(egui::Id::new("device_actions")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{ui.set_width(280.);ui.label(RichText::new(peer_name(&p)).size(18.).strong());muted(ui,format!("{} · {}",s(&p,"ipaddr"),route(&p)));if !s(&p,"ipaddr").is_empty()&&ui.button("Copy IP").clicked(){ctx.copy_text(s(&p,"ipaddr").into());self.notify("IP copied");self.action_peer=None;}if ui.button("Copy device ID").clicked(){ctx.copy_text(id.clone());self.notify("Device ID copied");}if !peer_ready(&p)&&!yes(&p,"declined")&&ui.add_enabled(!self.busy,egui::Button::new("Try again")).clicked(){self.job(Job::Retry(id.clone()));self.action_peer=None;}if !yes(&p,"confirmed")&&!yes(&p,"declined")&&ui.add_enabled(!self.busy,egui::Button::new("Cancel attempts")).clicked(){self.job(Job::Cancel(id.clone()));self.action_peer=None;}if ui.add_enabled(!self.busy,egui::Button::new("Forget device")).clicked(){self.job(Job::Remove(id.clone()));self.action_peer=None;}if ui.add_enabled(!self.busy,egui::Button::new("Remove & block")).clicked(){self.job(Job::Block(id.clone()));self.action_peer=None;}if ui.button("Close").clicked(){self.action_peer=None;}});if !open{self.action_peer=None;}}else{self.action_peer=None;}}
  let incoming=arr(&self.data.requests).to_vec();
  if !incoming.is_empty(){egui::Window::new(format!("Connection requests ({})",incoming.len())).id(egui::Id::new("incoming_requests")).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP,egui::vec2(-18.,48.)).show(ctx,|ui|{
   ui.set_width(280.);muted(ui,"Accept only devices you recognize.");
   for r in &incoming{let id=s(r,"peerid");let name=s(r,"name");if id.is_empty(){continue;}ui.separator();ui.label(RichText::new(if name.is_empty(){"Unknown device"}else{name}).size(17.).strong());muted(ui,format!("Device ID: {}…",id.chars().take(16).collect::<String>()));ui.add_enabled_ui(!self.busy,|ui|{ui.horizontal(|ui|{if primary(ui,"Accept"){self.job(Job::Accept(id.into(),if name.is_empty(){format!("Device {}",id.chars().rev().take(6).collect::<String>())}else{name.into()},false));}if ui.button("Decline & block").clicked(){self.job(Job::Accept(id.into(),String::new(),true));}});});}
  });}
  if let Some((id,since))=self.pending.clone(){
   let peer_owned=arr(&self.data.peers).iter().find(|p|s(p,"peerid")==id.as_str()).cloned();let peer=peer_owned.as_ref();
   let declined=peer.is_some_and(|p|yes(p,"declined"));
   let connected=peer.is_some_and(peer_ready);
   if connected||declined{self.pending=None;if connected{self.notify("Device connected. Virtual IP assigned.");}else{self.error="The remote device declined the request.".into();}}
   else if !self.pending_hidden {
    let stage=peer.map(route).unwrap_or("Saving invitation");
    let mut open=true;egui::Window::new("Connecting device").id(egui::Id::new("join_progress")).collapsible(false).resizable(false).open(&mut open).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
     ui.set_width(285.);ui.horizontal(|ui|{ui.spinner();ui.label(RichText::new(stage).size(19.).strong());});
     muted(ui,format!("{} seconds elapsed",since.elapsed().as_secs()));ui.separator();
     ui.label("Invitation saved locally");
     muted(ui,if peer.is_some_and(|p|yes(p,"confirmed")){"Authorized. Establishing the tunnel."}else if peer.is_some_and(|p|yes(p,"connected")){"Remote device reachable. Waiting for authorization."}else{"Discovering the remote device. Keep both clients online."});
     if since.elapsed()>Duration::from_secs(60){ui.colored_label(c(211,157,68),"Connection is taking longer");muted(ui,"Automatic retry enabled. Keep both devices online.");}if ui.add_enabled(!self.busy,egui::Button::new("Try again").min_size(egui::vec2(ui.available_width(),36.))).clicked(){self.job(Job::Retry(id.clone()));}if !peer.is_some_and(|p|yes(p,"confirmed"))&&ui.add_enabled(!self.busy,egui::Button::new("Cancel attempts")).clicked(){self.job(Job::Cancel(id.clone()));}
     if ui.add_sized([ui.available_width(),36.],egui::Button::new("Continue in background")).clicked(){self.pending_hidden=true;}
    });if !open{self.pending_hidden=true;}ctx.request_repaint_after(Duration::from_millis(100));
   }
  }
  if let Some(log)=self.logs.as_mut(){let mut open=true;egui::Window::new("Core logs").open(&mut open).resizable(true).show(ctx,|ui|{muted(ui,"Logs may contain private information. Review before sharing.");if ui.button("Copy logs").clicked(){ctx.copy_text(log.clone());}egui::ScrollArea::vertical().max_height(450.).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut log.as_str()).font(egui::TextStyle::Monospace).desired_width(650.));});});if !open{self.logs=None;}}
 }
}
fn peer_ready(p:&Value)->bool{yes(p,"confirmed")&&yes(p,"connected")&&!yes(p,"declined")&&!s(p,"ipaddr").is_empty()}
fn brand_icon(ui:&egui::Ui,r:egui::Rect){ui.painter().rect_filled(r,9,c(48,105,204));let a=r.min+egui::vec2(9.,9.);let b=r.min+egui::vec2(25.,9.);let d=r.min+egui::vec2(17.,25.);let st=egui::Stroke::new(1.7,Color32::WHITE);for pair in [[a,b],[a,d],[b,d]]{ui.painter().line_segment(pair,st);}for pt in [a,b,d]{ui.painter().circle_filled(pt,3.,Color32::WHITE);}}
fn main()->eframe::Result<()> {eframe::run_native("LiteLAN",eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_inner_size([440.,660.]).with_min_inner_size([440.,620.]).with_icon(std::sync::Arc::new(egui::IconData{rgba:include_bytes!("../assets/icon.rgba").to_vec(),width:64,height:64})),..Default::default()},Box::new(|cc|Ok(Box::new(App::new(cc))))) }
#[cfg(test)] mod tests{use super::*;#[test]fn parses_link(){let v=parse_invite("awl://invite?p=abc&n=My%20PC&t=secret","").unwrap();assert_eq!(v["peerID"],"abc");assert_eq!(v["alias"],"My PC");assert_eq!(v["token"],"secret");}#[test]fn actual_name_wins(){assert_eq!(peer_name(&json!({"remotename":"Remote-PC","alias":"Wrong alias"})),"Remote-PC");}#[test]fn rejects_http(){assert!(parse_invite("https://example.com","x").is_err());}#[test]fn go_json(){let me=normalize(json!({"VPN":{"IPNet":"10.64.160.184/11","VPNInterfaceEnabled":true},"ConnectedBootstrapPeers":4}));assert!(network_ready(&Snapshot{me,..Default::default()}));assert_eq!(ping(&normalize(json!({"Ping":18000000}))),"18 ms");assert_eq!(route(&normalize(json!({"Connected":true,"Confirmed":true,"IpAddr":"10.80.165.86","Connections":[{"ThroughRelay":false}]}))),"Direct");}}

#[cfg(test)] mod ui_state_tests { use super::*; #[test] fn pending_is_not_online(){assert!(!peer_ready(&json!({"connected":true,"confirmed":false,"ipaddr":"10.64.1.2"})));} #[test] fn declined_is_not_online(){assert!(!peer_ready(&json!({"connected":true,"confirmed":true,"declined":true,"ipaddr":"10.64.1.2"})));} #[test] fn no_ip_is_not_online(){assert!(!peer_ready(&json!({"connected":true,"confirmed":true})));} }
