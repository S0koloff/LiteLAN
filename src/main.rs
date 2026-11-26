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
fn decode_api_json(text:&str)->R<Value>{let v:Value=serde_json::from_str(text).map_err(err)?;let v=if let Some(inner)=v.as_str(){serde_json::from_str(inner).map_err(err)?}else{v};Ok(normalize(v))}
struct Api(reqwest::blocking::Client);
impl Api {
 fn new() -> R<Self> { Ok(Self(reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build().map_err(err)?)) }
 fn call(&self, path: &str, body: Option<Value>) -> R<String> {
  let u = format!("{BASE}{path}");
  let request = match body { Some(v) => self.0.post(u).json(&v), None => self.0.get(u) };
  let r = (if path=="/peers/litelan_prepare_invite" { request.timeout(Duration::from_secs(35)) } else { request }).send().map_err(err)?;
  let status = r.status(); let text = r.text().map_err(err)?;
  if !status.is_success() { return Err(format!("HTTP {status}: {text}")); } Ok(text)
 }
 fn get(&self, p: &str) -> R<Value> { decode_api_json(&self.call(p,None)?) }
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
enum Job {Start,Stop,Invite,Add(String,String),Accept(String,String,bool),Rename(String),Retry(String),Cancel(String),Remove(String),Block(String),Unblock(String),Logs,Reset,Exit}
enum Event {ResetDone,Removed(String,bool),Added(String),RetryQueued(String),Cancelled(String), ActionDone, State(String),Data(Snapshot),Invite(String),Logs(String),Error(String)}
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
   Ok(Job::Reset)=>{
    // Only reset our own profile, after our child has actually stopped.
    let result=(||->R<()> {
     let dir=profile()?;
     if let Some(c)=child.as_mut(){if c.try_wait().map_err(err)?.is_none(){c.kill().map_err(err)?;c.wait().map_err(err)?;}}
     child=None;
     send(Event::State("Stopped".into()));send(Event::Data(Snapshot::default()));
     if std::net::TcpStream::connect_timeout(&"127.0.0.1:18639".parse().unwrap(),Duration::from_millis(300)).is_ok(){return Err("Another LiteLAN instance is running. Close it before resetting.".into());}
     match fs::remove_dir_all(&dir){Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(format!("Could not clear local profile: {e}. Core remains stopped; retry reset."))}
     Ok(())
    })();
    match result{Ok(())=>{match start_core(){Ok(c)=>{child=Some(c);started=Instant::now();next=Instant::now();send(Event::ResetDone);send(Event::State("Starting core…".into()));},Err(e)=>{send(Event::ResetDone);send(Event::State("Core failed to start".into()));send(Event::Error(format!("Profile reset, but core could not restart: {e}")));}}},Err(e)=>{send(Event::Error(e));}}
   },
   Ok(Job::Logs)=>{let text=api.call("/debug/log?logs=100",None).or_else(|_|fs::read_to_string(profile()?.join("core.log")).map_err(err));match text{Ok(t)=>send(Event::Logs(t)),Err(e)=>send(Event::Error(e))}},
   Ok(job)=>{
    let res=match job {
     Job::Invite=>api.post_json("/peers/litelan_prepare_invite",json!({})).and_then(|routes|{
       let addresses=arr(&routes["addresses"]);if addresses.is_empty(){return Err("No verified IPv4 relay available. Try again shortly.".into())}
       api.post_json("/peers/invites/create",json!({"maxUses":1,"expiresInSeconds":86400,"allowUsingAsExitNode":false})).and_then(|v|{
        let link=s(&v,"link");if link.is_empty(){return Err("Invite response missing link".into())}
        let link=with_invite_routes(link,addresses)?;send(Event::Invite(link));Ok(String::new())
       })
     }),
     Job::Add(link,alias)=>parse_invite(&link,&alias).and_then(|mut v|{let id=v["peerID"].as_str().unwrap_or("").to_owned();api.get("/settings/peer_info").and_then(|me|{
      if s(&me,"peerid")==id{return Err("Cannot add your own device".into())}
      let routes=v.as_object_mut().and_then(|m|m.remove("addresses")).unwrap_or_else(||json!([]));
      if !arr(&routes).is_empty(){api.post("/peers/litelan_invite_routes",json!({"peerID":id,"addresses":routes}))?;}
      api.post("/peers/invite_peer",v).map_err(|e|if e.contains("peer has already been added"){"Device already saved. Open Overview and choose Try again, or remove it to use a new invitation.".into()}else{e})
     }).map(|t|{send(Event::Added(id));t})}),
     Job::Accept(id,alias,decline)=>api.post("/peers/accept_peer",json!({"peerID":id,"alias":alias,"decline":decline,"allowUsingAsExitNode":false})),
     Job::Rename(name)=>api.post("/settings/update",json!({"name":if name.trim().is_empty(){default_name()}else{name}})),
     Job::Retry(id)=>api.post("/peers/litelan_retry",json!({"peerID":id})).map(|t|{send(Event::RetryQueued(id));t}),
     Job::Cancel(id)=>api.post("/peers/litelan_cancel",json!({"peerID":id})).map(|t|{send(Event::Cancelled(id));t}),
     Job::Remove(id)=>api.post("/peers/remove",json!({"peerID":id})).and_then(|_|api.post("/peers/unblock",json!({"peerID":id})).map_err(|e|format!("Device removed; unblock failed: {e}. Open Connect > Blocked devices to unblock it."))).map(|t|{send(Event::Removed(id,false));t}),
     Job::Block(id)=>api.post("/peers/remove",json!({"peerID":id})).map(|t|{send(Event::Removed(id,true));t}),
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
 if link.len()>16384{return Err("Invitation is too long".into())}
 let u=url::Url::parse(link.trim()).map_err(err)?;if u.scheme()!="awl"||u.host_str()!=Some("invite"){return Err("Expected awl://invite link".into())}
 let (mut id,mut name,mut token)=(String::new(),String::new(),String::new());let mut addresses:Vec<String>=Vec::new();
 for (k,v) in u.query_pairs(){match k.as_ref(){"p"=>id=v.into_owned(),"n"=>name=v.into_owned(),"t"|"token"=>token=v.into_owned(),"a"=>{if addresses.len()>=16||v.len()>512{return Err("Invalid invitation route list".into())}addresses.push(v.into_owned());},_=>{}}}
 if id.is_empty(){return Err("Missing peer ID".into())} let alias=if !alias.trim().is_empty(){alias.trim().to_string()}else if !name.is_empty(){name}else{"Friend".into()};
 Ok(json!({"peerID":id,"alias":alias,"token":token,"allowUsingAsExitNode":false,"addresses":addresses}))
}
fn with_invite_routes(link:&str,addresses:&[Value])->R<String>{
 let mut u=url::Url::parse(link).map_err(err)?;
 if u.scheme()!="awl"||u.host_str()!=Some("invite"){return Err("Invalid core invitation".into())}
 {let mut q=u.query_pairs_mut();for a in addresses.iter().take(16){if let Some(a)=a.as_str(){q.append_pair("a",a);}}}
 Ok(u.to_string())
}
fn diagnostics(data:&Snapshot)->String{
 let peers:Vec<Value>=arr(&data.peers).iter().map(|p|json!({"peer_id":s(p,"peerid"),"name":peer_name(p),"confirmed":yes(p,"confirmed"),"connected":yes(p,"connected"),"declined":yes(p,"declined"),"stage":route(p),"connections":p["connections"],"cached_addresses":p["peerstoreaddrs"]})).collect();
 let d=&data.debug["dht"];
 serde_json::to_string_pretty(&json!({"litelan":env!("CARGO_PKG_VERSION"),"peer_id":s(&data.me,"peerid"),"reachability":s(&data.me,"reachability"),"bootstrap_connected":data.me["connectedbootstrappeers"],"dht_size":d["routingtablesize"],"listen_addresses":d["listenaddress"],"reachable_addresses":d["reachableaddrs"],"unreachable_addresses":d["unreachableaddrs"],"bootstrap":d["bootstrappeers"],"peers":peers})).unwrap_or_default()
}
struct App{tx:Sender<Job>,rx:Receiver<Event>,thread:Option<std::thread::JoinHandle<()>>,data:Snapshot,state:String,error:String,link:String,invite:String,name:String,logs:Option<String>,page:usize,dark:bool,busy:bool,pending:Option<(String,Instant)>,pending_hidden:bool,notice:String,notice_at:Instant,notice_kind:u8,name_loaded:bool,action_peer:Option<String>,confirm_reset:bool,confirm_block:Option<String>}
impl App{
 fn new(cc:&eframe::CreationContext<'_>)->Self{install_fonts(&cc.egui_ctx);let (tx,jobs)=mpsc::channel();let (events,rx)=mpsc::channel();let ctx=cc.egui_ctx.clone();let thread=std::thread::spawn(move||worker(jobs,events,ctx));let _=tx.send(Job::Start);Self{tx,rx,thread:Some(thread),data:Snapshot::default(),state:"Starting…".into(),error:String::new(),link:String::new(),invite:String::new(),name:String::new(),logs:None,page:0,dark:true,busy:false,pending:None,pending_hidden:false,notice:String::new(),notice_at:Instant::now(),notice_kind:0,name_loaded:false,action_peer:None,confirm_reset:false,confirm_block:None}}
 fn notify(&mut self,t:&str){self.notice=t.into();self.notice_at=Instant::now();self.notice_kind=if t.starts_with("Device connected")||t.contains("copied"){0}else{1};}
 fn job(&mut self,j:Job){self.error.clear();if matches!(&j,Job::Add(..)|Job::Invite|Job::Accept(..)|Job::Rename(..)|Job::Retry(..)|Job::Cancel(..)|Job::Remove(..)|Job::Block(..)|Job::Unblock(..)|Job::Reset){self.busy=true;self.notice.clear();}if self.tx.send(j).is_err(){self.busy=false;self.error="Worker unavailable".into();}}
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
 let (bg,panel,ink,sub,stroke,hover)=if dark{(c(18,22,30),c(25,30,40),c(237,241,248),c(152,165,184),c(48,57,71),c(35,45,61))}else{(c(245,247,251),Color32::WHITE,c(24,34,49),c(94,108,130),c(221,227,237),c(232,239,250))};
 v.panel_fill=bg;v.window_fill=panel;v.override_text_color=None;v.widgets.noninteractive.fg_stroke.color=ink;v.widgets.noninteractive.bg_stroke.color=stroke;v.widgets.noninteractive.weak_bg_fill=panel;v.selection.bg_fill=c(44,92,169);v.selection.stroke.color=Color32::WHITE;
 for w in [&mut v.widgets.inactive,&mut v.widgets.hovered,&mut v.widgets.active]{w.corner_radius=egui::CornerRadius::same(8);w.bg_stroke=egui::Stroke::new(1.,stroke);w.fg_stroke.color=ink;w.bg_fill=panel;w.weak_bg_fill=panel;}
 v.widgets.hovered.bg_fill=hover;v.widgets.hovered.weak_bg_fill=hover;v.widgets.active.bg_fill=hover;v.widgets.active.weak_bg_fill=hover;v.widgets.noninteractive.bg_fill=panel;v.faint_bg_color=hover;v.extreme_bg_color=bg;let _ = sub;
 style.visuals=v;style.spacing.item_spacing=egui::vec2(8.,8.);style.spacing.button_padding=egui::vec2(10.,6.);style.spacing.interact_size=egui::vec2(34.,36.);
 for (t,z) in [(egui::TextStyle::Body,14.),(egui::TextStyle::Button,13.),(egui::TextStyle::Heading,25.),(egui::TextStyle::Small,12.)]{style.text_styles.insert(t,egui::FontId::proportional(z));}ctx.set_style(style);
}
fn surface(ui:&mut egui::Ui,f:impl FnOnce(&mut egui::Ui)) {egui::Frame::new().fill(ui.visuals().window_fill).stroke(egui::Stroke::new(1.,ui.visuals().widgets.noninteractive.bg_stroke.color)).corner_radius(12).inner_margin(15).show(ui,|ui|{ui.set_width(ui.available_width());f(ui);});}
fn primary(ui:&mut egui::Ui,label:&str)->bool{ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE).strong()).fill(c(48,105,204)).stroke(egui::Stroke::NONE).corner_radius(8).min_size(egui::vec2(0.,36.))).clicked()}
fn input_field(ui:&mut egui::Ui, value:&mut String, hint:&str) {
 let width=ui.available_width();
 ui.add_sized([width,40.],egui::TextEdit::singleline(value).hint_text(hint).desired_width(width-20.).margin(egui::Margin::symmetric(10,9)).vertical_align(egui::Align::Center).min_size(egui::vec2(0.,40.)));
}
fn full_button(ui:&mut egui::Ui,label:&str,enabled:bool,accent:bool)->bool{
 let width=ui.available_width();
 ui.with_layout(egui::Layout::top_down(egui::Align::Center),|ui|{
  let text=if accent&&enabled{RichText::new(label).size(13.).color(Color32::WHITE)}else{RichText::new(label).size(13.)};
  let mut b=egui::Button::new(text).min_size(egui::vec2(width,40.)).corner_radius(8);
  if accent&&enabled {b=b.fill(c(48,105,204)).stroke(egui::Stroke::NONE);}
  ui.add_enabled(enabled,b).clicked()
 }).inner
}
fn action_icon(ui:&mut egui::Ui,r:egui::Rect)->bool{
 let response=ui.interact(r,ui.id().with("actions"),egui::Sense::click());
 let v=ui.style().interact(&response);
 ui.painter().rect_filled(r,9,v.weak_bg_fill);
 ui.painter().rect_stroke(r,9,v.bg_stroke,egui::StrokeKind::Inside);
 for dx in [-5.,0.,5.] {ui.painter().circle_filled(r.center()+egui::vec2(dx,0.),1.5,v.fg_stroke.color);}
 response.on_hover_text("Device actions").clicked()
}
fn dialog_frame(ctx:&egui::Context)->egui::Frame{egui::Frame::new().fill(ctx.style().visuals.window_fill).stroke(egui::Stroke::new(1.,ctx.style().visuals.widgets.noninteractive.bg_stroke.color)).corner_radius(14).inner_margin(16)}
fn dialog_heading(ui:&mut egui::Ui,title:&str)->bool{
 let (r,_)=ui.allocate_exact_size(egui::vec2(ui.available_width(),30.),egui::Sense::hover());
 layout_text(ui,lr(r,0.,0.,r.width()-32.,30.),title,20.,ui.visuals().text_color(),egui::Align2::LEFT_CENTER);
 let close=lr(r,r.width()-28.,0.,28.,28.);let response=ui.interact(close,ui.id().with("dialog_close"),egui::Sense::click());
 if response.hovered(){ui.painter().rect_filled(close,7,ui.visuals().widgets.hovered.weak_bg_fill);}menu_glyph(ui,close.center(),"cancel",ui.visuals().weak_text_color());response.on_hover_text("Close").clicked()
}
// All icons use the same 1.6-point stroke; no font glyphs or emoji.
fn curve(points:&mut Vec<egui::Pos2>,start:egui::Pos2,c1:egui::Pos2,c2:egui::Pos2,end:egui::Pos2){
 for i in 1..=12{let t=i as f32/12.;let u=1.-t;points.push(egui::pos2(u*u*u*start.x+3.*u*u*t*c1.x+3.*u*t*t*c2.x+t*t*t*end.x,u*u*u*start.y+3.*u*u*t*c1.y+3.*u*t*t*c2.y+t*t*t*end.y));}
}
fn menu_glyph(ui:&egui::Ui,pt:egui::Pos2,key:&str,color:Color32){
 let p=ui.painter();let st=egui::Stroke::new(1.6,color);let a=|x:f32,y:f32|pt+egui::vec2(x,y);
 let seg=|x:f32,y:f32,u:f32,v:f32|{p.line_segment([a(x,y),a(u,v)],st);};
 let path=|pts:&[(f32,f32)]|{p.add(egui::Shape::line(pts.iter().map(|&(x,y)|a(x,y)).collect(),st));};
 match key{
 "pc"=>{p.rect_stroke(egui::Rect::from_min_max(a(-9.,-8.),a(9.,4.)),2,st,egui::StrokeKind::Inside);seg(0.,4.,0.,8.);seg(-5.,8.,5.,8.);},
 "copy"=>{p.rect_stroke(egui::Rect::from_min_max(a(-3.,-3.),a(7.,7.)),2,st,egui::StrokeKind::Inside);let mut q=vec![a(-7.,3.),a(-7.,-5.)];curve(&mut q,a(-7.,-5.),a(-7.,-7.),a(-6.,-7.),a(-5.,-7.));q.push(a(2.,-7.));curve(&mut q,a(2.,-7.),a(3.,-7.),a(4.,-6.),a(4.,-5.));p.add(egui::Shape::line(q,st));},
 "id"=>{p.rect_stroke(egui::Rect::from_min_max(a(-9.,-7.),a(9.,7.)),2,st,egui::StrokeKind::Inside);p.circle_stroke(a(-4.,-2.),2.,st);let mut q=vec![a(-7.,4.)];curve(&mut q,a(-7.,4.),a(-7.,0.),a(-1.,0.),a(-1.,4.));p.add(egui::Shape::line(q,st));seg(2.,-2.,6.,-2.);seg(2.,2.,6.,2.);},
 "retry"=>{for (start,end) in [(-0.35_f32,2.5_f32),(2.79_f32,5.64_f32)]{let pts=(0..=24).map(|i|{let t=start+(end-start)*i as f32/24.;a(t.cos()*7.5,t.sin()*7.5)}).collect();p.add(egui::Shape::line(pts,st));}path(&[(7.,-8.),(7.,-3.),(2.,-3.)]);path(&[(-7.,8.),(-7.,3.),(-2.,3.)]);},
 "block"=>{p.circle_stroke(pt,8.,st);seg(-5.6,-5.6,5.6,5.6);},
 "remove"|"reset"=>{seg(-8.,-5.,8.,-5.);path(&[(-3.,-5.),(-3.,-8.),(3.,-8.),(3.,-5.)]);let mut q=vec![a(-5.5,-4.),a(-4.5,5.)];curve(&mut q,a(-4.5,5.),a(-4.5,7.),a(-3.5,7.),a(-2.5,7.));q.push(a(2.5,7.));curve(&mut q,a(2.5,7.),a(3.5,7.),a(4.5,7.),a(4.5,5.));q.push(a(5.5,-4.));p.add(egui::Shape::line(q,st));seg(-1.6,-2.,-1.6,4.);seg(1.6,-2.,1.6,4.);},
 _=>{seg(-4.,-4.,4.,4.);seg(-4.,4.,4.,-4.);}
 }
}
fn menu_action(ui:&mut egui::Ui,key:&str,label:&str,enabled:bool,danger:bool,accent:bool)->bool{
 menu_action_detail(ui,key,label,None,enabled,danger,accent)
}
fn menu_action_detail(ui:&mut egui::Ui,key:&str,label:&str,detail:Option<&str>,enabled:bool,danger:bool,accent:bool)->bool{
 let height=if detail.is_some(){58.}else{38.};
 let (r,_)=ui.allocate_exact_size(egui::vec2(ui.available_width(),height),egui::Sense::hover());
 let response=ui.interact(r,ui.id().with(key).with(label),if enabled{egui::Sense::click()}else{egui::Sense::hover()});
 let v=ui.style().interact(&response);let color=if !enabled{ui.visuals().weak_text_color()}else if danger{if ui.visuals().dark_mode{c(244,116,116)}else{c(177,38,38)}}else if accent{Color32::WHITE}else{ui.visuals().text_color()};
 let line=ui.visuals().widgets.noninteractive.bg_stroke.color;
 if accent&&enabled{ui.painter().rect_filled(r,8,if response.hovered(){c(65,122,222)}else{c(48,105,204)});}else if response.hovered()&&enabled{ui.painter().rect_filled(r,8,v.weak_bg_fill);}
 if danger||detail.is_some(){ui.painter().rect_stroke(r,8,egui::Stroke::new(1.,if danger{color.gamma_multiply(0.45)}else{line}),egui::StrokeKind::Inside);}
 let title_y=if detail.is_some(){7.}else{0.};let title_h=if detail.is_some(){23.}else{height};
 menu_glyph(ui,r.left_top()+egui::vec2(20.,title_y+title_h/2.),key,color);
 layout_text(ui,lr(r,38.,title_y,r.width()-48.,title_h),label,14.,color,egui::Align2::LEFT_CENTER);
 if let Some(detail)=detail{layout_text(ui,lr(r,38.,31.,r.width()-48.,19.),detail,12.,ui.visuals().weak_text_color(),egui::Align2::LEFT_CENTER);}
 enabled&&response.clicked()
}
fn section_caption(ui:&mut egui::Ui,title:&str){ui.add_space(5.);ui.label(RichText::new(title).size(12.).weak());}
fn muted(ui:&mut egui::Ui,t:impl Into<String>){ui.label(RichText::new(t.into()).size(13.).weak());}
fn appearance_toggle(ui:&mut egui::Ui,dark:&mut bool){
 let (r,response)=ui.allocate_exact_size(egui::vec2(ui.available_width(),36.),egui::Sense::click());
 if response.clicked(){*dark=!*dark;}
 let ink=ui.visuals().text_color();let sub=ui.visuals().weak_text_color();let p=ui.painter();let center=r.left_center()+egui::vec2(12.,0.);
 if *dark {let mut pts=vec![center+egui::vec2(3.,-8.)];let a=|x,y|center+egui::vec2(x,y);curve(&mut pts,a(3.,-8.),a(-8.,-8.),a(-11.,6.),a(-1.,8.));curve(&mut pts,a(-1.,8.),a(3.,9.),a(7.,6.),a(8.,2.));curve(&mut pts,a(8.,2.),a(1.,6.),a(-4.,-2.),a(3.,-8.));p.add(egui::Shape::line(pts,egui::Stroke::new(1.5,sub)));}
 else{p.circle_stroke(center,4.5,egui::Stroke::new(1.5,sub));for i in 0..8{let t=i as f32*std::f32::consts::FRAC_PI_4;let d=egui::vec2(t.cos(),t.sin());p.line_segment([center+d*6.5,center+d*9.],egui::Stroke::new(1.5,sub));}}
 layout_text(ui,lr(r,32.,0.,r.width()-88.,36.),"Dark appearance",14.,ink,egui::Align2::LEFT_CENTER);
 let track=egui::Rect::from_center_size(r.right_center()-egui::vec2(20.,0.),egui::vec2(40.,22.));
 p.rect_filled(track,11,if *dark{c(48,105,204)}else{ui.visuals().widgets.inactive.bg_stroke.color});
 p.circle_filled(track.center()+egui::vec2(if *dark{9.}else{-9.},0.),8.,Color32::WHITE);
 response.on_hover_text("Switch between light and dark appearance");
}
fn glyph_at(ui:&egui::Ui,r:egui::Rect,i:usize,color:Color32){
 let p=ui.painter();let st=egui::Stroke::new(1.6,color);let at=|x:f32,y:f32|r.min+egui::vec2(x*r.width()/20.,y*r.height()/20.);
 match i {
  0=>{for (x,y) in [(2.,2.),(11.,2.),(2.,11.),(11.,11.)]{p.rect_stroke(egui::Rect::from_min_max(at(x,y),at(x+7.,y+7.)),2,st,egui::StrokeKind::Inside);}},
  1=>{p.circle_stroke(at(7.,5.5),3.6,st);let mut q=vec![at(1.5,18.),at(1.5,15.5)];curve(&mut q,at(1.5,15.5),at(1.5,12.8),at(3.5,11.5),at(6.,11.5));q.push(at(8.,11.5));curve(&mut q,at(8.,11.5),at(10.5,11.5),at(12.5,12.8),at(12.5,15.5));q.push(at(12.5,18.));p.add(egui::Shape::line(q,st));p.line_segment([at(16.,5.),at(16.,13.)],st);p.line_segment([at(12.,9.),at(20.-0.8,9.)],st);},
  2=>{for (y,x) in [(4.,7.),(10.,13.),(16.,7.)]{p.line_segment([at(2.,y),at(x-2.5,y)],st);p.line_segment([at(x+2.5,y),at(18.,y)],st);p.circle_stroke(at(x,y),2.5,st);}},
  _=>{p.rect_stroke(egui::Rect::from_min_max(at(2.,3.),at(18.,17.)),3,st,egui::StrokeKind::Inside);p.add(egui::Shape::line(vec![at(5.,7.),at(9.,10.),at(5.,13.)],st));p.line_segment([at(11.,13.),at(15.,13.)],st);}
 }
}
fn num(v:&Value)->String{v.as_u64().map(|n|n.to_string()).unwrap_or_else(||"—".into())}
fn rate(v:&Value,k:&str)->String{let x=s(v,k);if x.is_empty(){"—".into()}else{x.into()}}
fn ping(p:&Value)->String{p["ping"].as_u64().filter(|n|*n>0).map(|n|format!("{:.0} ms",n as f64/1_000_000.)).unwrap_or_else(||"—".into())}
fn route(p:&Value)->&'static str{if yes(p,"declined"){return "Declined"}if !yes(p,"confirmed"){return if yes(p,"connected"){"Awaiting approval"}else{"Finding device"}}if !yes(p,"connected"){return "Offline"}if s(p,"ipaddr").is_empty(){return "Negotiating IP"}let con=arr(&p["connections"]);if con.iter().any(|c|c["throughrelay"].as_bool()==Some(false)&&!yes(c,"transient")){"Direct"}else if con.iter().any(|c|yes(c,"throughrelay")){"Relay"}else{"Connected"}}
fn peer_name(p:&Value)->String{let remote=s(p,"remotename");let alias=s(p,"alias");if !remote.trim().is_empty(){remote.into()}else if !alias.trim().is_empty(){alias.into()}else{let id=s(p,"peerid");format!("Device {}",id.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>())}}
fn status_color(p:&Value,dark:bool)->Color32{if yes(p,"declined"){c(229,105,105)}else if peer_ready(p){if route(p)=="Relay"{if dark{c(116,164,255)}else{c(34,86,174)}}else if dark{c(52,211,153)}else{c(16,128,83)}}else if !yes(p,"confirmed"){if dark{c(244,186,92)}else{c(162,99,16)}}else if dark{c(148,160,178)}else{c(94,108,130)}}
fn lr(r:egui::Rect,x:f32,y:f32,w:f32,h:f32)->egui::Rect{egui::Rect::from_min_size(r.min+egui::vec2(x,y),egui::vec2(w.max(1.),h.max(1.)))}
fn layout_text(ui:&egui::Ui,r:egui::Rect,text:&str,size:f32,color:Color32,align:egui::Align2){
 let pos=if align==egui::Align2::RIGHT_CENTER{r.right_center()}else if align==egui::Align2::CENTER_CENTER{r.center()}else{r.left_center()};
 let painter=ui.painter().with_clip_rect(r);painter.text(pos,align,text,egui::FontId::proportional(size),color);
}
fn layout_button(ui:&mut egui::Ui,r:egui::Rect,id:&str,text:&str,enabled:bool)->bool{
 let response=ui.interact(r,ui.id().with(id),if enabled{egui::Sense::click()}else{egui::Sense::hover()});let v=ui.style().interact(&response);
 ui.painter().rect_filled(r,8,v.weak_bg_fill);ui.painter().rect_stroke(r,8,v.bg_stroke,egui::StrokeKind::Inside);
 layout_text(ui,r,text,13.,if enabled{v.fg_stroke.color}else{ui.visuals().weak_text_color()},egui::Align2::CENTER_CENTER);enabled&&response.clicked()
}
fn pulse_at(ui:&egui::Ui,pt:egui::Pos2,color:Color32,animate:bool,r:f32){
 if animate{let t=(ui.input(|i|i.time)*0.5).fract() as f32;ui.painter().circle_stroke(pt,r+1.+5.*t,egui::Stroke::new(1.,color.gamma_multiply((1.-t)*0.7)));ui.ctx().request_repaint_after(Duration::from_millis(50));}ui.painter().circle_filled(pt,r,color);
}
fn traffic_arrow(ui:&egui::Ui,r:egui::Rect,down:bool,color:Color32){let p=r.center();let d=if down{1.}else{-1.};let st=egui::Stroke::new(1.4,color);for (a,b) in [(egui::vec2(0.,-5.*d),egui::vec2(0.,5.*d)),(egui::vec2(-3.5,1.5*d),egui::vec2(0.,5.*d)),(egui::vec2(0.,5.*d),egui::vec2(3.5,1.5*d))]{ui.painter().line_segment([p+a,p+b],st);}}
fn connection_icon(ui:&egui::Ui,r:egui::Rect,status:&str,color:Color32){
 let p=ui.painter();let st=egui::Stroke::new(1.5,color);let at=|x:f32,y:f32|r.min+egui::vec2(x*r.width()/20.,y*r.height()/20.);
 if status=="Direct"||status=="Connected"||status=="Offline" {
  // Two diagonal chain links, broken by a slash when offline.
  for pts in [[(9.,5.),(12.,2.),(15.,2.),(18.,5.),(18.,8.),(14.,12.),(11.,12.)],[(9.,8.),(6.,8.),(2.,12.),(2.,15.),(5.,18.),(8.,18.),(11.,15.)]]{p.add(egui::Shape::line(pts.iter().map(|(x,y)|at(*x,*y)).collect(),st));}
  if status=="Offline"{p.line_segment([at(2.,2.),at(18.,18.)],egui::Stroke::new(3.5,ui.visuals().panel_fill));p.line_segment([at(2.,2.),at(18.,18.)],st);}else{p.line_segment([at(7.,13.),at(13.,7.)],st);}
 }else if status=="Relay"{for (a,b) in [((2.,6.),(18.,6.)),((14.,2.),(18.,6.)),((18.,6.),(14.,10.)),((18.,14.),(2.,14.)),((6.,10.),(2.,14.)),((2.,14.),(6.,18.))]{p.line_segment([at(a.0,a.1),at(b.0,b.1)],st);}}
 else if status=="Declined"{p.circle_stroke(r.center(),7.,st);p.line_segment([at(5.,5.),at(15.,15.)],st);}
 else{p.circle_stroke(r.center(),7.,st);p.line_segment([at(10.,5.),at(10.,10.)],st);p.line_segment([at(10.,10.),at(13.,12.)],st);}
}
impl eframe::App for App{
 fn update(&mut self,ctx:&egui::Context,_:&mut eframe::Frame){
  while let Ok(e)=self.rx.try_recv(){match e{Event::ResetDone=>{self.data=Snapshot::default();self.state="Starting…".into();self.busy=false;self.error.clear();self.link.clear();self.invite.clear();self.name=default_name();self.name_loaded=false;self.logs=None;self.pending=None;self.pending_hidden=false;self.action_peer=None;self.confirm_reset=false;self.confirm_block=None;self.dark=true;self.page=0;self.notify("Application reset. New network identity will be created.");},Event::Removed(id,blocked)=>{if self.pending.as_ref().is_some_and(|(p,_)|p==&id){self.pending=None;}self.notify(if blocked{"Device removed and blocked"}else{"Device removed without blocking"});},Event::State(s)=>{if s=="Stopped"||s.starts_with("Core exited"){self.pending=None;}self.state=s;},Event::Data(d)=>{if !self.name_loaded && d.me.is_object(){self.name=s(&d.me,"name").into();if self.name.trim().is_empty(){self.name=default_name();}self.name_loaded=true;}let new_connected=arr(&d.peers).iter().any(|p|yes(p,"connected")&&yes(p,"confirmed")&&!s(p,"ipaddr").is_empty()&&!arr(&self.data.peers).iter().any(|old|s(old,"peerid")==s(p,"peerid")&&yes(old,"connected")&&yes(old,"confirmed")&&!s(old,"ipaddr").is_empty()));let disconnected=d.me.is_object()&&self.data.me.is_object()&&arr(&self.data.peers).iter().any(|old|peer_ready(old)&&arr(&d.peers).iter().any(|p|s(p,"peerid")==s(old,"peerid")&&!peer_ready(p)));if new_connected{self.notify("Device connected. Virtual IP assigned.");}else if disconnected{self.notify("Device disconnected. Reconnecting automatically.");self.notice_kind=2;}self.data=d;},Event::Error(e)=>{self.busy=false;self.error=e;},Event::ActionDone=>{self.busy=false;},Event::RetryQueued(id)=>{self.pending=Some((id,Instant::now()));self.pending_hidden=false;self.notify("Retry requested");},Event::Cancelled(id)=>{if self.pending.as_ref().is_some_and(|(p,_)|p==&id){self.pending=None;}self.notify("Connection attempts cancelled");},Event::Added(id)=>{self.link.clear();self.pending=Some((id,Instant::now()));self.pending_hidden=false;self.notify("Invitation saved. Finding device…");self.page=0;},Event::Invite(i)=>{ctx.copy_text(i.clone());self.invite=i;self.notify("Invitation copied. Share privately.");},Event::Logs(l)=>self.logs=Some(l)}}
  theme(ctx,self.dark);if (ctx.zoom_factor()-1.).abs()>0.01 {ctx.set_zoom_factor(1.);}
  let ready=network_ready(&self.data);let api=self.data.me.is_object();let ip=s(&self.data.me["vpn"],"ipnet").split('/').next().unwrap_or("").to_string();let online=arr(&self.data.peers).iter().filter(|p|yes(p,"connected")&&yes(p,"confirmed")&&!yes(p,"declined")&&!s(p,"ipaddr").is_empty()).count();let total=arr(&self.data.peers).len();
  let boot=format!("{} / {}",num(&self.data.me["connectedbootstrappeers"]),num(&self.data.me["totalbootstrappeers"]));let dht=num(&self.data.debug["dht"]["routingtablesize"]);let stats=&self.data.me["networkstatsiniecunits"];let rx=rate(stats,"ratein");let tx=rate(stats,"rateout");
  let active=api||self.state.starts_with("Starting")||self.state.starts_with("API unavailable");
  let dialog_open=self.action_peer.is_some()||self.confirm_reset||self.confirm_block.is_some();
  // Fixed header and navigation from the approved 440 × 660 layout.
  egui::TopBottomPanel::top("header").exact_height(60.).frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill)).show(ctx,|ui|{
   let (r,_)=ui.allocate_exact_size(egui::vec2(ui.available_width(),60.),egui::Sense::hover());
   brand_icon(ui,lr(r,16.,13.,34.,34.));
   layout_text(ui,lr(r,58.,14.,(r.width()-158.).max(1.),32.),"LiteLAN",21.,ui.visuals().text_color(),egui::Align2::LEFT_CENTER);
   layout_text(ui,lr(r,r.width()-98.,17.,82.,25.),concat!("v",env!("CARGO_PKG_VERSION")),12.,ui.visuals().weak_text_color(),egui::Align2::RIGHT_CENTER);
   ui.painter().line_segment([r.left_bottom()-egui::vec2(0.,0.5),r.right_bottom()-egui::vec2(0.,0.5)],egui::Stroke::new(1.,ui.visuals().widgets.noninteractive.bg_stroke.color));
  });
  egui::TopBottomPanel::bottom("nav").exact_height(72.).frame(egui::Frame::new().fill(if self.dark{c(23,28,37)}else{c(245,247,251)})).show(ctx,|ui|{
   let (r,_)=ui.allocate_exact_size(egui::vec2(ui.available_width(),72.),egui::Sense::hover());
   ui.painter().line_segment([r.left_top(),r.right_top()],egui::Stroke::new(1.,ui.visuals().widgets.noninteractive.bg_stroke.color));
   let width=((r.width()-14.-18.)/4.).max(1.);
   for (i,label) in ["Overview","Connect","Settings","Diagnostics"].iter().enumerate(){
    let b=lr(r,7.+i as f32*(width+6.),7.,width,58.);let response=ui.interact(b,ui.id().with(i),egui::Sense::click());let selected=self.page==i;
    let color=if selected{if self.dark{c(121,170,255)}else{c(34,86,174)}}else{ui.visuals().weak_text_color()};
    if selected||response.hovered(){ui.painter().rect_filled(b,9,if self.dark{c(32,48,73)}else{c(221,234,255)});}
    glyph_at(ui,egui::Rect::from_center_size(b.center_top()+egui::vec2(0.,17.),egui::vec2(20.,20.)),i,color);
    layout_text(ui,lr(b,0.,31.,width,22.),label,12.,color,egui::Align2::CENTER_CENTER);
    if !dialog_open&&response.clicked(){self.page=i;}
   }
  });
  egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(16)).show(ctx,|ui|{egui::ScrollArea::vertical().id_salt(self.page).auto_shrink([false,false]).show(ui,|ui|{
   if dialog_open{ui.disable();}
   ui.spacing_mut().item_spacing.y=0.;
   if self.page!=0{ui.spacing_mut().item_spacing.y=8.;}
   if self.busy {ui.horizontal(|ui|{ui.spinner();muted(ui,"Working…");});ui.add_space(8.);}
   if self.pending.is_some()&&self.pending_hidden {ui.horizontal_wrapped(|ui|{ui.spinner();muted(ui,"Connecting in background");if ui.button("Show progress").clicked(){self.pending_hidden=false;}});ui.add_space(8.);}
   match self.page {
    0=>{
     let width=ui.available_width();
     let ink=ui.visuals().text_color();let sub=ui.visuals().weak_text_color();let line=ui.visuals().widgets.noninteractive.bg_stroke.color;
     let (heading,_)=ui.allocate_exact_size(egui::vec2(width,58.),egui::Sense::hover());
     layout_text(ui,lr(heading,0.,0.,width,30.),"Overview",25.,ink,egui::Align2::LEFT_CENTER);
     layout_text(ui,lr(heading,0.,33.,width,18.),"Your private network",12.,sub,egui::Align2::LEFT_CENTER);
     ui.add_space(10.);
     let (net,_)=ui.allocate_exact_size(egui::vec2(width,188.),egui::Sense::hover());
     ui.painter().rect_filled(net,12,ui.visuals().window_fill);ui.painter().rect_stroke(net,12,egui::Stroke::new(1.,line),egui::StrokeKind::Inside);
     let k=(width/408.).min(1.);let shift=width-408.*k;
     let nr=|x:f32,y:f32,w:f32,h:f32|lr(net,x*k,y,w*k,h);
     let rr=|x:f32,y:f32,w:f32,h:f32|lr(net,x*k+shift,y,w*k,h);
     let state_color=if ready{c(52,211,153)}else if active{c(244,186,92)}else{sub};
     pulse_at(ui,nr(16.,20.,9.,9.).center(),state_color,ready||active,4.5);
     layout_text(ui,nr(34.,10.,210.,28.),if ready{"Network ready"}else if active{"Connecting"}else{"Disconnected"},15.,ink,egui::Align2::LEFT_CENTER);
     if layout_button(ui,rr(294.,10.,100.,34.),"network_toggle",if active{"Disconnect"}else{"Connect"},true){self.job(if active{Job::Stop}else{Job::Start});}
     layout_text(ui,nr(15.,52.,220.,15.),"YOUR VIRTUAL IP",11.,sub,egui::Align2::LEFT_CENTER);
     layout_text(ui,nr(15.,69.,265.,33.),if ip.is_empty(){"—"}else{&ip},23.,ink,egui::Align2::LEFT_CENTER);
     if layout_button(ui,rr(320.,66.,74.,36.),"copy_ip","Copy IP",!ip.is_empty()){ctx.copy_text(ip.clone());self.notify("IP copied");}
     ui.painter().line_segment([nr(15.,108.,1.,1.).left_top(),rr(393.,108.,1.,1.).left_top()],egui::Stroke::new(1.,line));
     layout_text(ui,nr(15.,118.,104.,18.),"Bootstrap",12.,sub,egui::Align2::LEFT_CENTER);
     layout_text(ui,nr(119.,118.,75.,18.),&boot,13.,ink,egui::Align2::RIGHT_CENTER);
     layout_text(ui,rr(215.,118.,90.,18.),"DHT nodes",12.,sub,egui::Align2::LEFT_CENTER);
     layout_text(ui,rr(309.,118.,84.,18.),&dht,13.,ink,egui::Align2::RIGHT_CENTER);
     traffic_arrow(ui,nr(15.,153.,14.,14.),true,sub);
     layout_text(ui,nr(33.,150.,65.,20.),"Down",12.,sub,egui::Align2::LEFT_CENTER);
     layout_text(ui,nr(100.,150.,94.,20.),&rx,13.,ink,egui::Align2::RIGHT_CENTER);
     traffic_arrow(ui,rr(215.,153.,14.,14.),false,sub);
     layout_text(ui,rr(233.,149.,67.,20.),"Up",12.,sub,egui::Align2::LEFT_CENTER);
     layout_text(ui,rr(300.,149.,93.,20.),&tx,13.,ink,egui::Align2::RIGHT_CENTER);
     if !ready && !self.state.is_empty(){ui.add_space(4.);muted(ui,self.state.clone());}
     ui.add_space(18.);
     let (head,_)=ui.allocate_exact_size(egui::vec2(width,38.),egui::Sense::hover());
     layout_text(ui,lr(head,0.,0.,170.,26.),"Devices",18.,ink,egui::Align2::LEFT_CENTER);
     layout_text(ui,lr(head,(width-174.).max(0.),0.,174.,26.),&format!("{online} online / {total}"),12.,sub,egui::Align2::RIGHT_CENTER);
     ui.painter().line_segment([head.left_bottom()-egui::vec2(0.,0.5),head.right_bottom()-egui::vec2(0.,0.5)],egui::Stroke::new(1.,line));
     ui.add_space(1.);
     if total==0{ui.vertical_centered(|ui|{ui.add_space(18.);ui.label(RichText::new("Your network is empty").strong());ui.add_space(6.);muted(ui,"Invite a friend to start playing together.");ui.add_space(12.);ui.add_enabled_ui(api&&!self.busy,|ui|{if primary(ui,"Invite a friend"){self.page=1;}});ui.add_space(18.);});}
     for p in arr(&self.data.peers).to_vec(){
      let id=s(&p,"peerid").to_owned();let address=s(&p,"ipaddr");let connected=peer_ready(&p);let status=route(&p);let color=status_color(&p,self.dark);
      ui.push_id(&id,|ui|{
       let (r,_)=ui.allocate_exact_size(egui::vec2(width,76.),egui::Sense::hover());
       let icon=lr(r,0.,15.,40.,40.);ui.painter().rect_filled(icon,10,ui.visuals().window_fill);
       let st=egui::Stroke::new(1.5,sub);let center=icon.center();ui.painter().rect_stroke(egui::Rect::from_center_size(center-egui::vec2(0.,3.),egui::vec2(18.,12.)),2,st,egui::StrokeKind::Inside);ui.painter().line_segment([center+egui::vec2(0.,3.),center+egui::vec2(0.,8.)],st);ui.painter().line_segment([center+egui::vec2(-5.,8.),center+egui::vec2(5.,8.)],st);
       let pt=lr(r,32.,47.,9.,9.).center();ui.painter().circle_filled(pt,6.,ui.visuals().panel_fill);pulse_at(ui,pt,color,connected&&status=="Direct",4.5);
       let menu=lr(r,width-36.,17.,36.,36.);let right=width-49.;let status_width=if status.len()>10{118.}else{65.};let status_left=right-status_width;let link_x=status_left-20.;let name_width=(link_x-56.).max(1.);
       layout_text(ui,lr(r,50.,12.,name_width,23.),&peer_name(&p),15.,ink,egui::Align2::LEFT_CENTER);
       layout_text(ui,lr(r,50.,37.,name_width,20.),if address.is_empty(){"No IP yet"}else{address},12.,sub,egui::Align2::LEFT_CENTER);
       connection_icon(ui,lr(r,link_x,13.,17.,17.),status,color);
       layout_text(ui,lr(r,status_left,11.,status_width,21.),status,12.,color,egui::Align2::RIGHT_CENTER);
       layout_text(ui,lr(r,right-102.,36.,102.,19.),&if connected{ping(&p)}else{"—".into()},12.,sub,egui::Align2::RIGHT_CENTER);
       if action_icon(ui,menu){self.action_peer=Some(id.clone());}
       ui.painter().line_segment([r.left_bottom()-egui::vec2(0.,0.5),r.right_bottom()-egui::vec2(0.,0.5)],egui::Stroke::new(1.,line));
      });
     }
    },
    1=>{
     ui.heading("Connect");muted(ui,"Connect a friend to your network");ui.add_space(8.);
     ui.add_enabled_ui(api,|ui|{
      surface(ui,|ui|{
       ui.label(RichText::new("Invite a friend").size(19.).strong());
       muted(ui,"Single-use link, valid for 24 hours. Share it with a friend to connect automatically.");
       if full_button(ui,"Create invitation",!self.busy,true){self.job(Job::Invite);}
       if !self.invite.is_empty(){muted(ui,"Invitation copied to clipboard.");}
      });
      surface(ui,|ui|{
       ui.label(RichText::new("Join a friend").size(19.).strong());
       muted(ui,"Invitation link");input_field(ui,&mut self.link,"awl://invite?…");
       if full_button(ui,"Connect",!self.busy&&!self.link.trim().is_empty(),true){self.job(Job::Add(self.link.clone(),String::new()));}
       let saved=s(&self.data.me,"name");let current=if saved.trim().is_empty(){default_name()}else{saved.to_owned()};
       ui.add_space(2.);
       ui.horizontal_wrapped(|ui|{ui.label(RichText::new("Your device name:").size(12.).weak());ui.label(RichText::new(current).size(12.).strong());});
       ui.horizontal_wrapped(|ui|{ui.label(RichText::new("Change it in").size(12.).weak());if ui.link(RichText::new("Settings").size(12.)).clicked(){self.page=2;}});
      });
      surface(ui,|ui|{ui.label(RichText::new("Blocked devices").strong());if self.data.blocked.is_null(){muted(ui,"Blocked list unavailable");}else if arr(&self.data.blocked).is_empty(){muted(ui,"No blocked devices");}for p in arr(&self.data.blocked).to_vec(){let id=s(&p,"peerid");let name=s(&p,"displayname");ui.horizontal_wrapped(|ui|{ui.label(if name.is_empty(){id}else{name});if !id.is_empty()&&ui.add_enabled(!self.busy,egui::Button::new("Unblock")).clicked(){self.job(Job::Unblock(id.into()));}});}});
     });
    },
    2=>{
     ui.heading("Settings");muted(ui,"Customize your workspace and device identity.");ui.add_space(8.);
     surface(ui,|ui|{ui.label(RichText::new("Appearance").size(19.).strong());appearance_toggle(ui,&mut self.dark);});
     surface(ui,|ui|{ui.label(RichText::new("Device name").size(19.).strong());ui.add_enabled_ui(api&&!self.busy,|ui|{input_field(ui,&mut self.name,"Computer name");if full_button(ui,"Save",!self.busy,true){if self.name.trim().is_empty(){self.name=default_name();}self.job(Job::Rename(self.name.trim().into()));}});muted(ui,"Private LAN only · No DNS, SOCKS5 or exit node");});
     surface(ui,|ui|{ui.label(RichText::new("Reset application").size(19.).strong());muted(ui,"Remove saved devices, network identity and local settings.");ui.add_space(4.);if menu_action(ui,"reset","Reset all data",!self.busy,true,false){self.confirm_reset=true;}});
    },
    _=>{ui.heading("Diagnostics");muted(ui,"Inspect the network core and connection health.");ui.add_space(12.);surface(ui,|ui|{ui.label(&self.state);ui.label(format!("Bootstrap: {boot} · DHT: {dht}"));muted(ui,format!("Reachability: {}",if s(&self.data.debug["dht"],"reachability").is_empty(){"Unknown"}else{s(&self.data.debug["dht"],"reachability")}));ui.label(format!("Adapter: {}",if yes(&self.data.me["vpn"],"vpninterfaceenabled"){"Enabled"}else{"Unavailable"}));ui.horizontal_wrapped(|ui|{if ui.add_enabled(!active,egui::Button::new("Start")).clicked(){self.job(Job::Start);}if ui.add_enabled(active,egui::Button::new("Stop")).clicked(){self.job(Job::Stop);}if primary(ui,"View core logs"){self.job(Job::Logs);}});ui.add_space(8.);if full_button(ui,"Copy diagnostics",api,true){ctx.copy_text(diagnostics(&self.data));self.notify("Diagnostics copied. Review before sharing.");}muted(ui,"Includes network addresses and device IDs; no invite tokens.");});}
   }
  });});
  if self.notice_at.elapsed()>Duration::from_secs(5){self.notice.clear();}
  if !self.notice.is_empty()||!self.error.is_empty(){
   let is_error=!self.error.is_empty();
   let text=if is_error{self.error.clone()}else{self.notice.clone()};
   let kind=if is_error{3}else{self.notice_kind};
   let toast_color=match kind{1=>c(116,164,255),2=>c(211,157,68),3=>c(229,105,105),_=>c(39,192,129)};
   let width=(ctx.screen_rect().width()-32.).clamp(220.,480.);
   egui::Area::new(egui::Id::new("toast_v18")).anchor(egui::Align2::CENTER_BOTTOM,egui::vec2(0.,-85.)).order(egui::Order::Foreground).movable(false).show(ctx,|ui|{
    let ink=ui.visuals().text_color();let weak=ui.visuals().weak_text_color();
    let body=if text.chars().count()>240{format!("{}…",text.chars().take(240).collect::<String>())}else{text.clone()};
    let galley=ui.painter().layout(body,egui::FontId::proportional(13.),weak,(width-82.).max(100.));
    let height=(galley.size().y+45.).max(64.);
    let (r,response)=ui.allocate_exact_size(egui::vec2(width,height),egui::Sense::hover());
    let painter=ui.painter();
    painter.rect_filled(r,11,ui.visuals().window_fill);painter.rect_stroke(r,11,egui::Stroke::new(1.,toast_color),egui::StrokeKind::Inside);
    let center=r.left_top()+egui::vec2(25.,24.);let st=egui::Stroke::new(1.7,toast_color);
    painter.circle_stroke(center,8.5,st);
    if kind==0{painter.line_segment([center+egui::vec2(-4.,0.),center+egui::vec2(-1.,3.)],st);painter.line_segment([center+egui::vec2(-1.,3.),center+egui::vec2(4.,-3.)],st);}else{painter.line_segment([center+egui::vec2(0.,-4.),center+egui::vec2(0.,1.)],st);painter.circle_filled(center+egui::vec2(0.,4.),1.,toast_color);}
    let title=match kind{1=>"Information",2=>"Connection notice",3=>"Something went wrong",_=>"Success"};
    painter.text(r.left_top()+egui::vec2(46.,13.),egui::Align2::LEFT_TOP,title,egui::FontId::proportional(14.),ink);
    painter.galley(r.left_top()+egui::vec2(46.,34.),galley,weak);
    let close_rect=egui::Rect::from_center_size(r.right_top()+egui::vec2(-18.,20.),egui::vec2(28.,28.));
    let close=ui.interact(close_rect,ui.id().with("close"),egui::Sense::click());
    if close.hovered(){ui.painter().rect_filled(close_rect,6,ui.visuals().widgets.hovered.weak_bg_fill);}
    let pt=close_rect.center();let st=egui::Stroke::new(1.5,weak);
    ui.painter().line_segment([pt+egui::vec2(-4.,-4.),pt+egui::vec2(4.,4.)],st);ui.painter().line_segment([pt+egui::vec2(-4.,4.),pt+egui::vec2(4.,-4.)],st);
    if close.on_hover_text("Dismiss").clicked(){self.notice.clear();self.error.clear();}
    if is_error{response.on_hover_text(text.clone());}
   });ctx.request_repaint_after(Duration::from_millis(250));
  }
  if let Some(id)=self.action_peer.clone(){
   if let Some(p)=arr(&self.data.peers).iter().find(|p|s(p,"peerid")==id.as_str()).cloned(){
    let frame=dialog_frame(ctx);
    egui::Window::new("Device actions").id(egui::Id::new("device_actions")).title_bar(false).collapsible(false).resizable(false).movable(false).frame(frame).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
     ui.set_width(324.);ui.spacing_mut().item_spacing.y=4.;
     if dialog_heading(ui,"Device actions"){self.action_peer=None;}
     ui.add_space(8.);
     let (r,_)=ui.allocate_exact_size(egui::vec2(ui.available_width(),58.),egui::Sense::hover());
     let icon=lr(r,0.,5.,44.,44.);let sub=ui.visuals().weak_text_color();let ink=ui.visuals().text_color();let color=status_color(&p,self.dark);
     ui.painter().rect_filled(icon,10,ui.visuals().faint_bg_color);menu_glyph(ui,icon.center(),"pc",sub);ui.painter().circle_filled(icon.right_bottom()-egui::vec2(5.,5.),4.5,color);
     layout_text(ui,lr(r,56.,0.,r.width()-56.,26.),&peer_name(&p),18.,ink,egui::Align2::LEFT_CENTER);
     layout_text(ui,lr(r,56.,27.,r.width()-56.,20.),&format!("{} · {}",if s(&p,"ipaddr").is_empty(){"No IP yet"}else{s(&p,"ipaddr")},route(&p)),12.,color,egui::Align2::LEFT_CENTER);
     ui.separator();
     section_caption(ui,"Quick actions");
     if menu_action(ui,"copy","Copy IP",!s(&p,"ipaddr").is_empty(),false,false){ctx.copy_text(s(&p,"ipaddr").into());self.notify("IP copied");self.action_peer=None;}
     if menu_action(ui,"id","Copy device ID",true,false,false){ctx.copy_text(id.clone());self.notify("Device ID copied");self.action_peer=None;}
     if !peer_ready(&p)&&!yes(&p,"declined") {
      ui.separator();section_caption(ui,"Connection");
      if menu_action(ui,"retry","Try again",!self.busy,false,true){self.job(Job::Retry(id.clone()));self.action_peer=None;}
      if !yes(&p,"confirmed")&&menu_action(ui,"cancel","Cancel attempts",!self.busy,false,false){self.job(Job::Cancel(id.clone()));self.action_peer=None;}
     }
     ui.add_space(5.);ui.separator();section_caption(ui,"Device management");
     if menu_action_detail(ui,"remove","Remove device",Some("Remove from your list without blocking."),!self.busy,false,false){self.job(Job::Remove(id.clone()));self.action_peer=None;}
     section_caption(ui,"Danger zone");
     if menu_action_detail(ui,"block","Remove & block",Some("Prevent future connection requests."),!self.busy,true,false){self.confirm_block=Some(id.clone());self.action_peer=None;}

    });
    if ctx.input(|i|i.key_pressed(egui::Key::Escape)){self.action_peer=None;}
   }else{self.action_peer=None;}
  }
  if self.confirm_reset {
   let frame=dialog_frame(ctx);
   egui::Window::new("Reset application").id(egui::Id::new("reset_confirm")).title_bar(false).collapsible(false).resizable(false).movable(false).frame(frame).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
    ui.set_width(324.);if dialog_heading(ui,"Reset application?"){self.confirm_reset=false;}
    ui.add_space(8.);ui.label("This permanently removes your saved devices, blocked list, invitations, network identity and local settings.");muted(ui,"Your device ID and virtual IP may change. Friends will need a new invitation. LiteLAN will restart its network core.");ui.add_space(8.);
    if menu_action(ui,"reset","Reset all data",!self.busy,true,false){self.confirm_reset=false;self.job(Job::Reset);}
    if full_button(ui,"Cancel",true,false){self.confirm_reset=false;}
   });if ctx.input(|i|i.key_pressed(egui::Key::Escape)){self.confirm_reset=false;}
  }
  if let Some(id)=self.confirm_block.clone(){
   let frame=dialog_frame(ctx);
   egui::Window::new("Block device").id(egui::Id::new("block_confirm")).title_bar(false).collapsible(false).resizable(false).movable(false).frame(frame).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
    ui.set_width(324.);if dialog_heading(ui,"Remove & block?"){self.confirm_block=None;}
    muted(ui,"The device will be removed and cannot send new requests until you unblock it in Connect.");
    if menu_action(ui,"block","Remove & block",!self.busy,true,false){self.confirm_block=None;self.job(Job::Block(id.clone()));}
    if full_button(ui,"Cancel",true,false){self.confirm_block=None;}
   });if ctx.input(|i|i.key_pressed(egui::Key::Escape)){self.confirm_block=None;}
  }
  let incoming=arr(&self.data.requests).to_vec();
  if !dialog_open&&!incoming.is_empty(){egui::Window::new(format!("Connection requests ({})",incoming.len())).id(egui::Id::new("incoming_requests")).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP,egui::vec2(-18.,48.)).show(ctx,|ui|{
   ui.set_width(280.);muted(ui,"Accept only devices you recognize.");
   for r in &incoming{let id=s(r,"peerid");let name=s(r,"name");if id.is_empty(){continue;}ui.separator();ui.label(RichText::new(if name.is_empty(){"Unknown device"}else{name}).size(17.).strong());muted(ui,format!("Device ID: {}…",id.chars().take(16).collect::<String>()));ui.add_enabled_ui(!self.busy,|ui|{ui.horizontal(|ui|{if primary(ui,"Accept"){self.job(Job::Accept(id.into(),if name.is_empty(){format!("Device {}",id.chars().rev().take(6).collect::<String>())}else{name.into()},false));}if ui.button("Decline & block").clicked(){self.job(Job::Accept(id.into(),String::new(),true));}});});}
  });}
  if let Some((id,since))=self.pending.clone(){
   let peer_owned=arr(&self.data.peers).iter().find(|p|s(p,"peerid")==id.as_str()).cloned();let peer=peer_owned.as_ref();
   let declined=peer.is_some_and(|p|yes(p,"declined"));
   let connected=peer.is_some_and(peer_ready);
   if connected||declined{self.pending=None;if connected{self.notify("Device connected. Virtual IP assigned.");}else{self.error="The remote device declined the request.".into();}}
   else if !dialog_open&&!self.pending_hidden {
    let stage=if peer.is_some_and(|p|yes(p,"confirmed")){"Establishing tunnel"}else if peer.is_some_and(|p|yes(p,"connected")){"Authorizing invitation"}else{"Connecting to friend"};
    let friend=peer.map(peer_name).unwrap_or_else(||"Friend".into());
    let mut open=true;egui::Window::new("Joining friend’s network").id(egui::Id::new("join_progress")).collapsible(false).resizable(false).open(&mut open).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
     ui.set_width(285.);ui.horizontal(|ui|{ui.spinner();ui.label(RichText::new(stage).size(19.).strong());});
     muted(ui,format!("{} seconds elapsed",since.elapsed().as_secs()));ui.separator();
     ui.label(RichText::new(&friend).strong());
     muted(ui,"Invitation saved locally");
     muted(ui,if peer.is_some_and(|p|yes(p,"confirmed")){"Authorized. Establishing the tunnel."}else if peer.is_some_and(|p|yes(p,"connected")){"Remote device reachable. Waiting for authorization."}else{"Trying known addresses, relay and DHT. Keep both clients online."});
     if since.elapsed()>Duration::from_secs(60){ui.colored_label(c(211,157,68),"Connection is taking longer");muted(ui,"Automatic retries are enabled. If it stays offline, ask for a fresh invitation or open Diagnostics.");}if full_button(ui,"Try again",!self.busy,true){self.job(Job::Retry(id.clone()));}if !peer.is_some_and(|p|yes(p,"confirmed"))&&ui.add_enabled(!self.busy,egui::Button::new("Cancel attempts")).clicked(){self.job(Job::Cancel(id.clone()));}
     if full_button(ui,"Continue in background",true,false){self.pending_hidden=true;}
    });if !open{self.pending_hidden=true;}ctx.request_repaint_after(Duration::from_millis(100));
   }
  }
  if let Some(log)=self.logs.as_mut(){let mut open=true;egui::Window::new("Core logs").open(&mut open).resizable(true).show(ctx,|ui|{muted(ui,"Logs may contain private information. Review before sharing.");if ui.button("Copy logs").clicked(){ctx.copy_text(log.clone());}egui::ScrollArea::vertical().max_height(450.).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut log.as_str()).font(egui::TextStyle::Monospace).desired_width(650.));});});if !open{self.logs=None;}}
 }
}
fn peer_ready(p:&Value)->bool{yes(p,"confirmed")&&yes(p,"connected")&&!yes(p,"declined")&&!s(p,"ipaddr").is_empty()}
fn brand_icon(ui:&egui::Ui,r:egui::Rect){ui.painter().rect_filled(r,9,c(48,105,204));let a=r.min+egui::vec2(9.,9.);let b=r.min+egui::vec2(25.,9.);let d=r.min+egui::vec2(17.,25.);let st=egui::Stroke::new(1.7,Color32::WHITE);for pair in [[a,b],[a,d],[b,d]]{ui.painter().line_segment(pair,st);}for pt in [a,b,d]{ui.painter().circle_filled(pt,3.,Color32::WHITE);}}
fn main()->eframe::Result<()> {eframe::run_native("LiteLAN",eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_inner_size([440.,660.]).with_min_inner_size([440.,660.]).with_icon(std::sync::Arc::new(egui::IconData{rgba:include_bytes!("../assets/icon.rgba").to_vec(),width:64,height:64})),..Default::default()},Box::new(|cc|Ok(Box::new(App::new(cc))))) }
#[cfg(test)] mod tests{use super::*;#[test]fn parses_link(){let v=parse_invite("awl://invite?p=abc&n=My%20PC&t=secret","").unwrap();assert_eq!(v["peerID"],"abc");assert_eq!(v["alias"],"My PC");assert_eq!(v["token"],"secret");}#[test]fn actual_name_wins(){assert_eq!(peer_name(&json!({"remotename":"Remote-PC","alias":"Wrong alias"})),"Remote-PC");}#[test]fn rejects_http(){assert!(parse_invite("https://example.com","x").is_err());}#[test]fn go_json(){let me=normalize(json!({"VPN":{"IPNet":"10.64.160.184/11","VPNInterfaceEnabled":true},"ConnectedBootstrapPeers":4}));assert!(network_ready(&Snapshot{me,..Default::default()}));assert_eq!(ping(&normalize(json!({"Ping":18000000}))),"18 ms");assert_eq!(route(&normalize(json!({"Connected":true,"Confirmed":true,"IpAddr":"10.80.165.86","Connections":[{"ThroughRelay":false}]}))),"Direct");}}

#[cfg(test)] mod ui_state_tests { use super::*; #[test] fn pending_is_not_online(){assert!(!peer_ready(&json!({"connected":true,"confirmed":false,"ipaddr":"10.64.1.2"})));} #[test] fn declined_is_not_online(){assert!(!peer_ready(&json!({"connected":true,"confirmed":true,"declined":true,"ipaddr":"10.64.1.2"})));} #[test] fn no_ip_is_not_online(){assert!(!peer_ready(&json!({"connected":true,"confirmed":true})));} }

#[cfg(test)] mod invite_route_tests {
 use super::*;
 #[test] fn route_round_trip(){
  let addresses=vec![json!("/ip4/8.8.8.8/tcp/4363"),json!("/ip6/2001:4860::1/udp/4363/quic-v1")];
  let link=with_invite_routes("awl://invite?p=abc&n=Home-PC&t=secret",&addresses).unwrap();
  let v=parse_invite(&link,"").unwrap();assert_eq!(v["token"],"secret");assert_eq!(v["addresses"],json!(addresses));
 }
 #[test] fn old_invite_works(){assert_eq!(parse_invite("awl://invite?p=abc&n=Home-PC&t=secret","").unwrap()["addresses"],json!([]));}
 #[test] fn rejects_excessive_routes(){let link=format!("awl://invite?p=abc{}","&a=x".repeat(17));assert!(parse_invite(&link,"").is_err());}
 #[test] fn parses_string_wrapped_debug(){let inner=r#"{"DHT":{"RoutingTableSize":72}}"#;let wrapped=serde_json::to_string(inner).unwrap();assert_eq!(decode_api_json(&wrapped).unwrap()["dht"]["routingtablesize"],72);}
}
