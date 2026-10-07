use eframe::egui::{self, Key, Modifiers};
use std::path::PathBuf;
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Binding { pub key: Key, pub ctrl: bool, pub alt: bool, pub shift: bool }
impl Binding {
    pub fn new(key: Key) -> Self { Self { key, ctrl:false,alt:false,shift:false } }
    pub fn label(self) -> String {format!("{}{}{}{}",if self.ctrl{"Ctrl+"}else{""},if self.alt{"Alt+"}else{""},if self.shift{"Shift+"}else{""},self.key.name())}
    pub fn pressed(self,ctx:&egui::Context)->bool {ctx.input(|i|i.key_pressed(self.key)&&i.modifiers.ctrl==self.ctrl&&i.modifiers.alt==self.alt&&i.modifiers.shift==self.shift)}
}
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(usize)]
pub enum Action {Next,Previous,First,Last,Fit,ActualSize,PlayPause,Fullscreen,Back,Forward,Up,Dual,SwitchPane,Settings,NextAlternate,PreviousAlternate,NewTab,CloseTab,DuplicateTab,LockTab}
pub const ACTIONS: [Action;20]=[Action::Next,Action::Previous,Action::First,Action::Last,Action::Fit,Action::ActualSize,Action::PlayPause,Action::Fullscreen,Action::Back,Action::Forward,Action::Up,Action::Dual,Action::SwitchPane,Action::Settings,Action::NextAlternate,Action::PreviousAlternate,Action::NewTab,Action::CloseTab,Action::DuplicateTab,Action::LockTab];
impl Action {pub fn label(self)-> &'static str {match self {Self::Next=>"Next file / PDF page",Self::Previous=>"Previous file / PDF page",Self::First=>"First file / PDF page",Self::Last=>"Last file / PDF page",Self::Fit=>"Fit / reset 3D view",Self::ActualSize=>"Actual size",Self::PlayPause=>"Play / pause video",Self::Fullscreen=>"Toggle fullscreen",Self::Back=>"Navigate back",Self::Forward=>"Navigate forward",Self::Up=>"Parent folder",Self::Dual=>"Toggle dual panes",Self::SwitchPane=>"Switch active pane",Self::Settings=>"Open settings",Self::NextAlternate=>"Next file / page (alternate)",Self::PreviousAlternate=>"Previous file / page (alternate)",Self::NewTab=>"New tab",Self::CloseTab=>"Close tab",Self::DuplicateTab=>"Duplicate tab",Self::LockTab=>"Lock / unlock tab"}}}
#[derive(Clone)]
pub struct Settings { pub bindings:Vec<Option<Binding>>,pub thumb_size:f32,pub cache:bool,pub gpu:bool,pub dual:bool,pub invert_x:bool,pub invert_y:bool,pub orbit_speed:f32,pub zoom_speed:f32 }
impl Default for Settings {
    fn default()->Self {
        let mut bindings:Vec<_>=[Key::ArrowRight,Key::ArrowLeft,Key::Home,Key::End,Key::F,Key::Num1,Key::Space,Key::F11,Key::ArrowLeft,Key::ArrowRight,Key::ArrowUp,Key::F6,Key::Tab,Key::Comma,Key::PageDown,Key::PageUp,Key::T,Key::W,Key::D,Key::L].into_iter().map(|key|Some(Binding::new(key))).collect();
        for action in [Action::Back,Action::Forward,Action::Up]{bindings[action as usize].as_mut().unwrap().alt=true;}
        bindings[Action::Settings as usize].as_mut().unwrap().ctrl=true;
        for action in [Action::NewTab,Action::CloseTab,Action::DuplicateTab,Action::LockTab]{bindings[action as usize].as_mut().unwrap().ctrl=true;} bindings[Action::LockTab as usize].as_mut().unwrap().shift=true;
        Self{bindings,thumb_size:160.0,cache:true,gpu:true,dual:false,invert_x:true,invert_y:true,orbit_speed:0.01,zoom_speed:0.002}
    }
}
impl Settings {
    pub fn path()->PathBuf{std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("ArchView/settings.conf")}
    pub fn pressed(&self,action:Action,ctx:&egui::Context)->bool{self.bindings[action as usize].is_some_and(|b|b.pressed(ctx))}
    pub fn load()->Self{std::fs::read_to_string(Self::path()).map(|text|Self::parse(&text)).unwrap_or_default()}
    pub fn parse(text:&str)->Self {
        let mut result=Self::default();
        for line in text.lines(){let Some((key,value))=line.split_once('=')else{continue;};
            match key {
                "thumb_size"=>{if let Ok(v)=value.parse::<f32>(){if v.is_finite(){result.thumb_size=v.clamp(80.0,320.0);}}}
                "cache"=>result.cache=value=="true","gpu"=>result.gpu=value=="true","dual"=>result.dual=value=="true",
                "invert_x"=>result.invert_x=value=="true","invert_y"=>result.invert_y=value=="true",
                "orbit_speed"=>{if let Ok(v)=value.parse::<f32>(){if v.is_finite(){result.orbit_speed=v.clamp(0.002,0.03);}}}
                "zoom_speed"=>{if let Ok(v)=value.parse::<f32>(){if v.is_finite(){result.zoom_speed=v.clamp(0.0005,0.01);}}}
                _=>if let Some(index)=key.strip_prefix("key_").and_then(|v|v.parse::<usize>().ok()).filter(|i|*i<ACTIONS.len()) {
                    if value=="none"{result.bindings[index]=None;}else{
                        let parts:Vec<_>=value.split('|').collect();
                        if parts.len()==4 {if let Some(key)=Key::from_name(parts[0]){result.bindings[index]=Some(Binding{key,ctrl:parts[1]=="1",alt:parts[2]=="1",shift:parts[3]=="1"});}}
                    }
                }
            }
        }result
    }
    pub fn text(&self)->String {
        let mut text=format!("thumb_size={}\ncache={}\ngpu={}\ndual={}\ninvert_x={}\ninvert_y={}\norbit_speed={}\nzoom_speed={}\n",self.thumb_size,self.cache,self.gpu,self.dual,self.invert_x,self.invert_y,self.orbit_speed,self.zoom_speed);
        for (i,b) in self.bindings.iter().enumerate(){text.push_str(&format!("key_{i}={}\n",b.map(|b|format!("{}|{}|{}|{}",b.key.name(),u8::from(b.ctrl),u8::from(b.alt),u8::from(b.shift))).unwrap_or_else(||"none".into())));}text
    }
    pub fn save(&self)->anyhow::Result<()>{let path=Self::path();std::fs::create_dir_all(path.parent().unwrap())?;let temporary=path.with_extension("tmp");std::fs::write(&temporary,self.text())?;std::fs::rename(temporary,path)?;Ok(())}
    pub fn assign(&mut self,action:Action,binding:Binding)->Result<(),String>{
        if binding.key==Key::Escape{return Err("Escape is reserved for closing fullscreen and cancelling key capture.".into());}
        if let Some(other)=ACTIONS.iter().find(|other|**other!=action&&self.bindings[**other as usize]==Some(binding)){return Err(format!("Already assigned to {}",other.label()));}
        self.bindings[action as usize]=Some(binding);Ok(())
    }
    pub fn window(&mut self,ctx:&egui::Context,open:&mut bool,capture:&mut Option<Action>,message:&mut String)->bool{
        let mut changed=false;
        if let Some(action)=*capture {
            let key=ctx.input(|i|i.events.iter().find_map(|event|if let egui::Event::Key{key,pressed:true,repeat:false,modifiers,..}=event{Some((*key,*modifiers))}else{None}));
            if let Some((key,Modifiers{ctrl,alt,shift,..}))=key {
                if key==Key::Escape{*capture=None;message.clear();}else{match self.assign(action,Binding{key,ctrl,alt,shift}){Ok(())=>{changed=true;*capture=None;message.clear();},Err(error)=>*message=error}}
            }
        }
        egui::Window::new("Settings").open(open).default_width(520.0).show(ctx,|ui|{
            egui::ScrollArea::vertical().max_height(650.0).show(ui,|ui|{
                ui.heading("Viewing");
                changed|=ui.add(egui::Slider::new(&mut self.thumb_size,80.0..=320.0).text("Default thumbnail size").suffix(" px")).changed();
                changed|=ui.checkbox(&mut self.cache,"Cache thumbnails").changed();
                changed|=ui.checkbox(&mut self.gpu,"GPU video decoding").changed();
                changed|=ui.checkbox(&mut self.dual,"Dual panes").changed();
                ui.separator();ui.heading("3D controls");
                changed|=ui.checkbox(&mut self.invert_x,"Invert horizontal orbit").changed();
                changed|=ui.checkbox(&mut self.invert_y,"Invert vertical orbit").changed();
                changed|=ui.add(egui::Slider::new(&mut self.orbit_speed,0.002..=0.03).text("Orbit sensitivity")).changed();
                changed|=ui.add(egui::Slider::new(&mut self.zoom_speed,0.0005..=0.01).text("Zoom sensitivity")).changed();
                ui.separator();ui.heading("Keyboard shortcuts");
                ui.label("Click a shortcut, then press the new key combination. Escape cancels capture.");
                egui::Grid::new("hotkey_grid").striped(true).show(ui,|ui|{for action in ACTIONS{
                    ui.label(action.label());
                    let label=if *capture==Some(action){"Press keys…".into()}else{self.bindings[action as usize].map(Binding::label).unwrap_or_else(||"Unassigned".into())};
                    if ui.button(label).clicked(){*capture=Some(action);message.clear();}
                    if ui.small_button("Clear").clicked(){self.bindings[action as usize]=None;changed=true;}
                    ui.end_row();
                }});
                if !message.is_empty(){ui.colored_label(egui::Color32::YELLOW,message.as_str());}
                ui.separator();
                if ui.button("Restore defaults").clicked(){*self=Self::default();*capture=None;message.clear();changed=true;}
                ui.label(format!("Saved automatically: {}",Self::path().display()));
            });
        });
        if !*open{*capture=None;}changed
    }
}
#[cfg(test)]mod tests{use super::*;#[test]fn bindings_and_preferences_roundtrip_and_conflicts_are_rejected(){let mut s=Settings::default();s.dual=true;s.invert_y=false;assert!(s.assign(Action::Next,Binding::new(Key::Space)).is_err());let binding=Binding{key:Key::N,ctrl:true,alt:false,shift:true};s.assign(Action::Next,binding).unwrap();let loaded=Settings::parse(&s.text());assert_eq!(loaded.bindings,s.bindings);assert!(loaded.dual);assert!(!loaded.invert_y);assert!(s.assign(Action::Next,Binding::new(Key::Escape)).is_err());}}

