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
pub enum Action {Next,Previous,First,Last,Fit,ActualSize,PlayPause,Fullscreen,Back,Forward,Up,Dual,SwitchPane,Settings,NextAlternate,PreviousAlternate,NewTab,CloseTab,DuplicateTab,LockTab,CopyFiles,CutFiles,PasteFiles,DuplicateFiles,RenameFile,Address,SelectAllFiles,DetailsView,DeleteFiles}
pub const ACTIONS: [Action;29]=[Action::Next,Action::Previous,Action::First,Action::Last,Action::Fit,Action::ActualSize,Action::PlayPause,Action::Fullscreen,Action::Back,Action::Forward,Action::Up,Action::Dual,Action::SwitchPane,Action::Settings,Action::NextAlternate,Action::PreviousAlternate,Action::NewTab,Action::CloseTab,Action::DuplicateTab,Action::LockTab,Action::CopyFiles,Action::CutFiles,Action::PasteFiles,Action::DuplicateFiles,Action::RenameFile,Action::Address,Action::SelectAllFiles,Action::DetailsView,Action::DeleteFiles];
impl Action {pub fn label(self)-> &'static str {match self {Self::Next=>"Next file / PDF page",Self::Previous=>"Previous file / PDF page",Self::First=>"First file / PDF page",Self::Last=>"Last file / PDF page",Self::Fit=>"Fit / reset 3D view",Self::ActualSize=>"Actual size",Self::PlayPause=>"Play / pause video",Self::Fullscreen=>"Toggle fullscreen",Self::Back=>"Navigate back",Self::Forward=>"Navigate forward",Self::Up=>"Parent folder",Self::Dual=>"Toggle dual panes",Self::SwitchPane=>"Switch active pane",Self::Settings=>"Open settings",Self::NextAlternate=>"Next file / page (alternate)",Self::PreviousAlternate=>"Previous file / page (alternate)",Self::NewTab=>"New tab",Self::CloseTab=>"Close tab",Self::DuplicateTab=>"Duplicate tab",Self::LockTab=>"Lock / unlock tab",Self::CopyFiles=>"Copy files",Self::CutFiles=>"Cut files",Self::PasteFiles=>"Paste files",Self::DuplicateFiles=>"Duplicate files",Self::RenameFile=>"Rename file",Self::Address=>"Focus address bar",Self::SelectAllFiles=>"Select all files",Self::DetailsView=>"Toggle Details view",Self::DeleteFiles=>"Delete files"}}}
#[derive(Clone)]
pub struct Settings { pub collections_root:String, pub photo_preload:bool,pub photo_ram_mb:usize,pub photo_gpu_mb:usize, pub fullscreen_fraction:f32, pub compare_fullscreen:bool,pub comfortable:bool,pub show_tree:bool,pub show_preview:bool,pub review:bool,pub split_preview:bool,pub tree_width:f32,pub preview_width:f32,pub pane_fraction:f32,pub preview_fraction:f32, pub scrub_ram_mb:usize,pub preload_video:bool,pub cuda_preload:bool, pub bindings:Vec<Option<Binding>>,pub thumb_size:f32,pub cache:bool,pub gpu:bool,pub dual:bool,pub invert_x:bool,pub invert_y:bool,pub orbit_speed:f32,pub zoom_speed:f32 }
impl Default for Settings {
    fn default()->Self {
        let mut bindings:Vec<_>=[Key::ArrowRight,Key::ArrowLeft,Key::Home,Key::End,Key::F,Key::Num1,Key::Space,Key::F11,Key::ArrowLeft,Key::ArrowRight,Key::ArrowUp,Key::F6,Key::Tab,Key::Comma,Key::PageDown,Key::PageUp,Key::T,Key::W,Key::D,Key::L].into_iter().map(|key|Some(Binding::new(key))).collect();
        for action in [Action::Back,Action::Forward,Action::Up]{bindings[action as usize].as_mut().unwrap().alt=true;}
        bindings[Action::Settings as usize].as_mut().unwrap().ctrl=true;
        for action in [Action::NewTab,Action::CloseTab,Action::DuplicateTab,Action::LockTab]{bindings[action as usize].as_mut().unwrap().ctrl=true;} bindings[Action::LockTab as usize].as_mut().unwrap().shift=true;
        for (key,ctrl,shift) in [(Key::C,true,false),(Key::X,true,false),(Key::V,true,false),(Key::D,true,true),(Key::F2,false,false),(Key::L,true,false),(Key::A,true,false),(Key::F4,false,false),(Key::Delete,false,false)] {bindings.push(Some(Binding{key,ctrl,alt:false,shift}));}
        Self{collections_root:String::new(),photo_preload:true,photo_ram_mb:crate::image_pipeline::default_ram_mb(),photo_gpu_mb:1024,fullscreen_fraction:0.5,compare_fullscreen:false,comfortable:false,show_tree:true,show_preview:true,review:false,split_preview:false,tree_width:240.0,preview_width:340.0,pane_fraction:0.5,preview_fraction:0.5,scrub_ram_mb:crate::video_cache::default_mb(),preload_video:crate::video_cache::default_mb()>=8192,cuda_preload:true,bindings,thumb_size:160.0,cache:true,gpu:true,dual:false,invert_x:true,invert_y:true,orbit_speed:0.01,zoom_speed:0.002}
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
                "collections_root"=>result.collections_root=serde_json::from_str(value).unwrap_or_default(),
                "photo_preload"=>result.photo_preload=value=="true",
                "photo_ram_mb"=>{if let Ok(v)=value.parse::<usize>(){result.photo_ram_mb=v.min(16384);}},
                "photo_gpu_mb"=>{if let Ok(v)=value.parse::<usize>(){result.photo_gpu_mb=v.min(4096);}},
                "compare_fullscreen"=>result.compare_fullscreen=value=="true",
                "comfortable"=>result.comfortable=value=="true","show_tree"=>result.show_tree=value=="true","show_preview"=>result.show_preview=value=="true","review"=>result.review=value=="true","split_preview"=>result.split_preview=value=="true",
                "fullscreen_fraction"=>{if let Ok(v)=value.parse::<f32>(){if v.is_finite(){result.fullscreen_fraction=v.clamp(0.15,0.85);}}},
                "tree_width"|"preview_width"|"pane_fraction"|"preview_fraction"=>{if let Ok(v)=value.parse::<f32>(){if v.is_finite(){match key{"tree_width"=>result.tree_width=v.clamp(160.0,600.0),"preview_width"=>result.preview_width=v.clamp(200.0,1000.0),"pane_fraction"=>result.pane_fraction=v.clamp(0.2,0.8),_=>result.preview_fraction=v.clamp(0.2,0.8)}}}},
                "scrub_ram_mb"=>{if let Ok(v)=value.parse::<usize>(){result.scrub_ram_mb=v.min(32768);}}
                "preload_video"=>result.preload_video=value=="true","cuda_preload"=>result.cuda_preload=value=="true",
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
        text.push_str(&format!("scrub_ram_mb={}\npreload_video={}\ncuda_preload={}\n",self.scrub_ram_mb,self.preload_video,self.cuda_preload));
        text.push_str(&format!("photo_preload={}\nphoto_ram_mb={}\nphoto_gpu_mb={}\n",self.photo_preload,self.photo_ram_mb,self.photo_gpu_mb));
        text.push_str(&format!("compare_fullscreen={}\n",self.compare_fullscreen));
        text.push_str(&format!("fullscreen_fraction={}\n",self.fullscreen_fraction));
        text.push_str(&format!("comfortable={}\nshow_tree={}\nshow_preview={}\nreview={}\nsplit_preview={}\ntree_width={}\npreview_width={}\npane_fraction={}\npreview_fraction={}\n",self.comfortable,self.show_tree,self.show_preview,self.review,self.split_preview,self.tree_width,self.preview_width,self.pane_fraction,self.preview_fraction));
        text.push_str(&format!("collections_root={}\n",serde_json::to_string(&self.collections_root).unwrap()));
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
                ui.heading("Collections");
                ui.label(crate::collections::storage::root().display().to_string());
                if ui.button("Change collection folder…").clicked(){if let Some(path)=rfd::FileDialog::new().set_title("Choose an empty folder for collections").pick_folder(){ctx.data_mut(|d|d.insert_temp(egui::Id::new("collection_root_request"),path));}}
                ui.small("Collections are canvases. Imported images are stored beside their canvas. Changing the folder copies the library; the old copy is kept.");
                ui.separator();ui.heading("Appearance");
                ui.horizontal(|ui|{ui.label("Density");changed|=ui.selectable_value(&mut self.comfortable,false,"Compact").changed();changed|=ui.selectable_value(&mut self.comfortable,true,"Comfortable").changed();});
                changed|=ui.checkbox(&mut self.show_tree,"Folder sidebar").changed();changed|=ui.checkbox(&mut self.show_preview,"Preview panel").changed();
                ui.separator();ui.heading("Viewing");
                changed|=ui.add(egui::Slider::new(&mut self.thumb_size,80.0..=320.0).text("Default thumbnail size").suffix(" px")).changed();
                changed|=ui.checkbox(&mut self.cache,"Cache thumbnails").changed();
                if ui.button("Clear thumbnail cache").clicked(){*message=match crate::thumbnail_cache::clear(){Ok(count)=>format!("Removed {count} cached thumbnails"),Err(error)=>format!("Cache: {error}")};}
                changed|=ui.checkbox(&mut self.gpu,"GPU video decoding").changed();
                changed|=ui.checkbox(&mut self.dual,"Dual panes").changed();
                ui.separator();ui.heading("Photo performance");
                changed|=ui.checkbox(&mut self.photo_preload,"Preload neighboring photos").changed();
                changed|=ui.add(egui::Slider::new(&mut self.photo_ram_mb,0..=16384).logarithmic(true).text("Shared photo RAM cache").suffix(" MB")).changed();
                changed|=ui.add(egui::Slider::new(&mut self.photo_gpu_mb,0..=4096).logarithmic(true).text("Shared GPU texture cache").suffix(" MB")).changed();
                ui.label("Shared across panes and tabs. GPU budget includes photo mip levels; current previews and driver overhead are additional. Decoding runs on two CPU workers.");
                let (ram,ram_limit,gpu,gpu_limit)=crate::image_pipeline::stats(ctx);
                ui.label(format!("Photo RAM: {ram} / {ram_limit} MB · GPU textures: {gpu} / {gpu_limit} MB"));
                if ui.button("Clear photo caches").clicked(){crate::image_pipeline::clear(ctx);}
                ui.separator();ui.heading("Video performance");
                changed|=ui.add(egui::Slider::new(&mut self.scrub_ram_mb,0..=32768).logarithmic(true).text("Shared scrub RAM budget").suffix(" MB")).changed();
                ui.label("0 disables the shared RAM cache. The budget is shared across both panes and all tabs.");
                changed|=ui.checkbox(&mut self.preload_video,"Preload scrub frames in background").changed();
                changed|=ui.checkbox(&mut self.cuda_preload,"NVIDIA CUDA decode / resize for preload").changed();
                ui.label("Unsupported codecs, rotation or CUDA builds use software. Preload reserves 25% of the budget for precise seeks.");
                let (used,budget,frames,jobs,cuda)=crate::video_cache::stats();
                ui.label(format!("RAM: {used} / {budget} MB · {frames} frames · {jobs} preload jobs · {cuda} CUDA frames"));
                if ui.button("Clear scrub RAM cache").clicked(){crate::video_cache::clear();}
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
                if ui.button("Restore defaults").clicked(){let root=self.collections_root.clone();*self=Self::default();self.collections_root=root;*capture=None;message.clear();changed=true;}
                ui.label(format!("Saved automatically: {}",Self::path().display()));
            });
        });
        if !*open{*capture=None;}changed
    }
}
#[cfg(test)]mod tests{use super::*;#[test]fn bindings_and_preferences_roundtrip_and_conflicts_are_rejected(){let mut s=Settings::default();s.collections_root=r"D:\Референсы\Collection Library".into();s.dual=true;s.invert_y=false;assert!(s.assign(Action::Next,Binding::new(Key::Space)).is_err());let binding=Binding{key:Key::N,ctrl:true,alt:false,shift:true};s.assign(Action::Next,binding).unwrap();let loaded=Settings::parse(&s.text());assert_eq!(loaded.bindings,s.bindings);assert_eq!(loaded.collections_root,s.collections_root);assert!(loaded.dual);assert_eq!(loaded.scrub_ram_mb,s.scrub_ram_mb);assert_eq!(loaded.preload_video,s.preload_video);assert_eq!(loaded.cuda_preload,s.cuda_preload);assert_eq!(Settings::parse("scrub_ram_mb=999999").scrub_ram_mb,32768);assert!(!loaded.invert_y);assert!(s.assign(Action::Next,Binding::new(Key::Escape)).is_err());}}


#[cfg(test)]mod layout_tests {
 use super::*;
 #[test]fn workspace_preferences_roundtrip_and_reject_nonfinite_sizes(){
  let mut original=Settings::default();original.comfortable=true;original.review=true;original.show_tree=false;original.show_preview=false;original.split_preview=true;original.dual=true;original.tree_width=280.0;original.preview_width=460.0;original.pane_fraction=0.35;original.preview_fraction=0.65;
  let loaded=Settings::parse(&original.text());
  assert!(loaded.comfortable&&loaded.review&&loaded.split_preview&&loaded.dual);assert!(!loaded.show_tree&&!loaded.show_preview);
  assert_eq!((loaded.tree_width,loaded.preview_width,loaded.pane_fraction,loaded.preview_fraction),(280.0,460.0,0.35,0.65));
  let invalid=Settings::parse("tree_width=NaN\npreview_width=inf\npane_fraction=-1\npreview_fraction=99");
  assert_eq!(invalid.tree_width,Settings::default().tree_width);assert_eq!(invalid.preview_width,Settings::default().preview_width);assert_eq!(invalid.pane_fraction,0.2);assert_eq!(invalid.preview_fraction,0.8);
 }
}
