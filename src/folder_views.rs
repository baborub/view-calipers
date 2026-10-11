
//! Per-folder view records, independent of pane and tab lifetime.
use crate::{file_browser::{Sort,SortKey},settings};
use std::{collections::BTreeMap,path::{Path,PathBuf},sync::Mutex};
pub const KEYS:[SortKey;5]=[SortKey::Name,SortKey::Type,SortKey::Size,SortKey::Created,SortKey::Modified];
#[derive(Clone,PartialEq)]
pub struct Column {pub key:SortKey,pub width:f32,pub visible:bool}
#[derive(Clone,PartialEq)]
pub struct Layout {pub columns:Vec<Column>}
impl Default for Layout {fn default()->Self{Self{columns:KEYS.into_iter().zip([270.0,95.0,100.0,190.0,190.0]).map(|(key,width)|Column{key,width,visible:true}).collect()}}}
impl Layout {
    pub fn visible(&self)->Vec<Column>{self.columns.iter().filter(|c|c.visible).cloned().collect()}
    pub fn resize(&mut self,key:SortKey,delta:f32){if delta.is_finite(){if let Some(c)=self.columns.iter_mut().find(|c|c.key==key){c.width=(c.width+delta).clamp(60.0,1200.0);}}}
    pub fn reorder(&mut self,source:SortKey,target:SortKey){if source==target{return;}let Some(from)=self.columns.iter().position(|c|c.key==source)else{return;};let Some(to)=self.columns.iter().position(|c|c.key==target)else{return;};let column=self.columns.remove(from);self.columns.insert(to,column);}
}
#[derive(Clone,PartialEq)]
pub struct View {pub details:bool,pub sort:Sort,pub thumb:f32,pub layout:Layout}
fn identity(folder:&Path)->String{folder.to_string_lossy().replace('/',"\\").trim_end_matches('\\').to_lowercase()}
fn hex(text:&str)->String{text.as_bytes().iter().map(|b|format!("{b:02x}")).collect()}
fn unhex(text:&str)->Option<String>{if text.len()%2!=0||!text.is_ascii(){return None;}let bytes=(0..text.len()).step_by(2).map(|i|u8::from_str_radix(&text[i..i+2],16).ok()).collect::<Option<Vec<_>>>()?;String::from_utf8(bytes).ok()}
pub(super) fn parse(text:&str)->BTreeMap<String,View>{text.lines().filter_map(|line|{
    let parts:Vec<_>=line.split('\t').collect();if parts.len()!=6{return None;}
    let folder=unhex(parts[0])?;let details=parts[1].parse().ok()?;let key=*KEYS.get(parts[2].parse::<usize>().ok()?)?;let descending=parts[3].parse().ok()?;
    let thumb=parts[4].parse::<f32>().ok()?;if !thumb.is_finite(){return None;}
    let mut columns=Vec::new();for value in parts[5].split(','){let field:Vec<_>=value.split(':').collect();if field.len()!=3{return None;}let key=*KEYS.get(field[0].parse::<usize>().ok()?)?;if columns.iter().any(|c:&Column|c.key==key){return None;}let width=field[1].parse::<f32>().ok()?;if !width.is_finite(){return None;}let visible=field[2].parse().ok()?;columns.push(Column{key,width:width.clamp(60.0,1200.0),visible:visible||key==SortKey::Name});}
    if columns.len()!=5{return None;}
    Some((folder,View{details,sort:Sort{key,descending},thumb:thumb.clamp(80.0,320.0),layout:Layout{columns}}))
}).collect()}
pub(super) fn encode(records:&BTreeMap<String,View>)->String {let mut text=String::from("# view-calipers folder views v1\n");for (folder,v) in records {let columns=v.layout.columns.iter().map(|c|format!("{}:{}:{}",c.key as u8,c.width,c.visible)).collect::<Vec<_>>().join(",");text.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\n",hex(folder),v.details,v.sort.key as u8,v.sort.descending,v.thumb,columns));}text}
pub fn path()->PathBuf{if cfg!(test){std::env::temp_dir().join(format!("view-calipers-folder-views-test-{}.tsv",std::process::id()))}else{settings::Settings::path().with_file_name("folder-views.tsv")}}
static STORE:Mutex<()>=Mutex::new(());
pub fn load(folder:&Path)->Option<View>{let _lock=STORE.lock().ok()?;parse(&std::fs::read_to_string(path()).ok()?).remove(&identity(folder))}
pub fn save(folder:&Path,view:&View)->anyhow::Result<()> {save_to(&path(),folder,view)}
fn save_to(file:&Path,folder:&Path,view:&View)->anyhow::Result<()> {
    let _lock=STORE.lock().map_err(|_|anyhow::anyhow!("Folder view storage is unavailable"))?;
    let mut records=match std::fs::read_to_string(file){Ok(text)=>parse(&text),Err(error) if error.kind()==std::io::ErrorKind::NotFound=>BTreeMap::new(),Err(error)=>return Err(error.into())};records.insert(identity(folder),view.clone());
    if let Some(parent)=file.parent(){std::fs::create_dir_all(parent)?;}
    let temporary=file.with_extension("tmp");std::fs::write(&temporary,encode(&records))?;std::fs::rename(&temporary,file)?;Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn view()->View{View{details:true,sort:Sort{key:SortKey::Modified,descending:true},thumb:240.0,layout:Layout::default()}}
    #[test]fn columns_resize_reorder_and_record_roundtrip(){let mut v=view();v.layout.resize(SortKey::Name,200.0);v.layout.resize(SortKey::Size,-500.0);assert_eq!(v.layout.columns[2].width,60.0);v.layout.reorder(SortKey::Modified,SortKey::Name);v.layout.columns[2].visible=false;let mut records=BTreeMap::new();records.insert(identity(Path::new("C:/Projects/Тест")),v.clone());let loaded=parse(&encode(&records));assert!(loaded.values().next()==Some(&v));assert_eq!(v.layout.visible()[0].key as u8,SortKey::Modified as u8);}
    #[test]fn storage_preserves_other_folders_and_handles_corrupt_records(){let file=std::env::temp_dir().join(format!("view-calipers-views-{}.tsv",std::process::id()));let a=Path::new("C:/Project/A");let b=Path::new("C:/Project/B");let va=view();let mut vb=view();vb.details=false;vb.thumb=80.0;save_to(&file,a,&va).unwrap();save_to(&file,b,&vb).unwrap();let records=parse(&std::fs::read_to_string(&file).unwrap());assert!(records.get(&identity(a))==Some(&va));assert!(records.get(&identity(b))==Some(&vb));assert!(parse("broken\tNaN\n").is_empty());assert_eq!(identity(Path::new("c:\\PROJECT\\A\\")),identity(a));std::fs::remove_file(file).unwrap();}
}
