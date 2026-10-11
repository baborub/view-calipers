//! Non-destructive reference-board operations, independent of UI and decoding.
use super::*;
#[cfg(test)]
mod tests;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct ObjectOptions {
    pub locked: bool,
    pub group: Option<u64>,
    pub opacity: f32,
    pub grayscale: bool,
    pub flip_x: bool,
    pub flip_y: bool,
    pub crop: [f32; 4],
}
impl Default for ObjectOptions {
    fn default() -> Self {
        Self {
            locked: false,
            group: None,
            opacity: 1.0,
            grayscale: false,
            flip_x: false,
            flip_y: false,
            crop: [0.0, 0.0, 1.0, 1.0],
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct CanvasOptions {
    pub locked: bool,
    pub grayscale: bool,
    pub grid: bool,
    pub snap: bool,
    pub step: f32,
    pub gap: f32,
}
impl Default for CanvasOptions {
    fn default() -> Self {
        Self {
            locked: false,
            grayscale: false,
            grid: true,
            snap: false,
            step: 25.0,
            gap: 24.0,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Pack,
    Grid,
    Row,
    Column,
    Align(u8),
    Normalize(u8),
    Duplicate,
    Group,
    Ungroup,
    ToggleLock,
    UnlockAll,
    Front,
    Back,
    FlipX,
    FlipY,
    Grayscale,
    Opacity(f32),
    ResetCrop,
}
impl Command {
    pub fn layout(self) -> bool {
        matches!(
            self,
            Self::Pack
                | Self::Grid
                | Self::Row
                | Self::Column
                | Self::Align(_)
                | Self::Normalize(_)
        )
    }
}
impl Object {
    pub fn visual_media(&self) -> bool {
        matches!(
            self.content,
            Content::Image { .. } | Content::ArchiveImage { .. } | Content::Video { .. }
        )
    }
    pub fn label(&self) -> String {
        match &self.content {
            Content::Image { source }
            | Content::Video { source }
            | Content::Markdown { source } => file_name(source),
            Content::ArchiveImage { entry, .. } => file_name(Path::new(entry)),
            Content::Text { text, .. } => text
                .lines()
                .next()
                .unwrap_or("Note")
                .chars()
                .take(80)
                .collect(),
            Content::Ink { .. } => "Drawing".into(),
        }
    }
    pub fn display_size(&self, texture: [usize; 2]) -> Vec2 {
        let c = self.options.crop;
        let source = Vec2::new(
            texture[0] as f32 * (c[2] - c[0]),
            texture[1] as f32 * (c[3] - c[1]),
        );
        source * (self.size[0] / source.x.max(0.001)).min(self.size[1] / source.y.max(0.001))
    }
    pub fn effects(&self, grayscale: bool) -> media_gpu::Effects {
        let mut uv = self.options.crop;
        if self.options.flip_x {
            uv.swap(0, 2);
        }
        if self.options.flip_y {
            uv.swap(1, 3);
        }
        media_gpu::Effects {
            uv,
            opacity: self.options.opacity,
            grayscale: grayscale || self.options.grayscale,
        }
    }
    /// A crop rectangle in the currently displayed image, before object rotation.
    pub fn crop_to(&mut self, display: Rect, texture: [usize; 2]) -> bool {
        let r = display.intersect(Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
        if !self.visual_media() || r.width() < 0.005 || r.height() < 0.005 {
            return false;
        }
        let size = self.display_size(texture);
        let c = self.options.crop;
        if size.x * r.width() < 0.1
            || size.y * r.height() < 0.1
            || (c[2] - c[0]) * r.width() < 0.00002
            || (c[3] - c[1]) * r.height() < 0.00002
        {
            return false;
        }
        let delta = rotate((r.center() - Pos2::new(0.5, 0.5)) * size, self.angle);
        self.center[0] += delta.x;
        self.center[1] += delta.y;
        self.size = [size.x * r.width(), size.y * r.height()];
        let mut u = [r.left(), r.top(), r.right(), r.bottom()];
        if self.options.flip_x {
            (u[0], u[2]) = (1.0 - u[2], 1.0 - u[0]);
        }
        if self.options.flip_y {
            (u[1], u[3]) = (1.0 - u[3], 1.0 - u[1]);
        }
        self.options.crop = [
            c[0] + u[0] * (c[2] - c[0]),
            c[1] + u[1] * (c[3] - c[1]),
            c[0] + u[2] * (c[2] - c[0]),
            c[1] + u[3] * (c[3] - c[1]),
        ];
        true
    }
    fn reset_crop(&mut self) {
        let c = self.options.crop;
        let size = Vec2::new(self.size[0] / (c[2] - c[0]), self.size[1] / (c[3] - c[1]));
        let mut delta = Vec2::new(
            (c[0] + c[2] - 1.0) * 0.5 * size.x,
            (c[1] + c[3] - 1.0) * 0.5 * size.y,
        );
        if self.options.flip_x {
            delta.x = -delta.x;
        }
        if self.options.flip_y {
            delta.y = -delta.y;
        }
        let delta = rotate(delta, self.angle);
        self.center[0] -= delta.x;
        self.center[1] -= delta.y;
        self.size = [size.x.min(1e7), size.y.min(1e7)];
        self.options.crop = [0.0, 0.0, 1.0, 1.0];
    }
}
struct Unit {
    indices: Vec<usize>,
    bounds: Rect,
}
pub(super) fn group_rect(bounds: Rect, zoom: f32) -> Rect {
    let mut rect = bounds.expand(18.0);
    rect.min.y -= 20.0 / zoom;
    rect
}
impl Document {
    pub fn validate_options(&self) -> Result<()> {
        anyhow::ensure!(
            self.groups.len() <= MAX_OBJECTS
                && self
                    .groups
                    .iter()
                    .all(|(id, name)| *id < u64::MAX && name.len() <= 8192),
            "Invalid groups"
        );
        anyhow::ensure!(
            self.canvas.step.is_finite()
                && (1.0..=1000.0).contains(&self.canvas.step)
                && self.canvas.gap.is_finite()
                && (0.0..=1000.0).contains(&self.canvas.gap),
            "Invalid canvas spacing"
        );
        for o in &self.objects {
            let p = &o.options;
            anyhow::ensure!(
                p.opacity.is_finite()
                    && (0.0..=1.0).contains(&p.opacity)
                    && p.crop
                        .iter()
                        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                    && p.crop[2] - p.crop[0] >= 0.00001
                    && p.crop[3] - p.crop[1] >= 0.00001
                    && p.group.is_none_or(|id| self.groups.contains_key(&id)),
                "Invalid object options"
            );
        }
        Ok(())
    }
    pub fn upgrade_options(&mut self) {
        if self.canvas != CanvasOptions::default()
            || !self.groups.is_empty()
            || self
                .objects
                .iter()
                .any(|o| o.options != ObjectOptions::default())
        {
            self.version = self.version.max(3);
        }
    }
    pub fn clean_groups(&mut self) {
        let used: HashSet<_> = self
            .objects
            .iter()
            .filter_map(|o| o.options.group)
            .collect();
        self.groups.retain(|id, _| used.contains(id));
    }
    pub fn selection_for(&self, id: u64, individual: bool) -> HashSet<u64> {
        let Some(object) = self.objects.iter().find(|o| o.id == id) else {
            return HashSet::new();
        };
        if let Some(group) = object.options.group.filter(|_| !individual) {
            self.objects
                .iter()
                .filter(|o| o.options.group == Some(group))
                .map(|o| o.id)
                .collect()
        } else {
            HashSet::from([id])
        }
    }
    #[cfg(test)]
    pub fn group_bounds(&self, id: u64) -> Option<Rect> {
        self.objects
            .iter()
            .filter(|o| o.options.group == Some(id))
            .map(Object::bounds)
            .reduce(Rect::union)
    }
    pub fn apply(&mut self, selected: &mut HashSet<u64>, command: Command, aspect: f32) -> bool {
        if self.canvas.locked && !matches!(command, Command::UnlockAll) {
            return false;
        }
        let all = selected.is_empty() && command.layout();
        let indices: Vec<usize> = self
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                (all || selected.contains(&o.id))
                    && (!o.options.locked || matches!(command, Command::ToggleLock))
            })
            .map(|(i, _)| i)
            .collect();
        if matches!(command, Command::UnlockAll) {
            self.canvas.locked = false;
            for o in &mut self.objects {
                o.options.locked = false;
            }
        } else if indices.is_empty() {
            return false;
        } else if command.layout() {
            self.layout(&indices, command, aspect);
        } else {
            match command {
                Command::Duplicate => {
                    if self.objects.len() + indices.len() > MAX_OBJECTS {
                        return false;
                    }
                    let mut next = self.next_id();
                    let mut groups = HashMap::new();
                    let mut next_group = self.groups.keys().max().copied().unwrap_or(0) + 1;
                    if next >= u64::MAX - indices.len() as u64
                        || next_group >= u64::MAX - indices.len() as u64
                    {
                        return false;
                    }
                    let mut copies = Vec::new();
                    for i in indices {
                        let mut o = self.objects[i].clone();
                        o.id = next;
                        next += 1;
                        o.center[0] += 30.0;
                        o.center[1] += 30.0;
                        if let Some(g) = o.options.group {
                            let new = *groups.entry(g).or_insert_with(|| {
                                let n = next_group;
                                next_group += 1;
                                n
                            });
                            self.groups.insert(new, format!("{} copy", self.groups[&g]));
                            o.options.group = Some(new);
                        }
                        copies.push(o);
                    }
                    *selected = copies.iter().map(|o| o.id).collect();
                    self.objects.extend(copies);
                }
                Command::Group => {
                    if indices.len() < 2 {
                        return false;
                    }
                    let id = self.groups.keys().max().copied().unwrap_or(0) + 1;
                    if id == u64::MAX {
                        return false;
                    }
                    self.groups.insert(id, format!("Group {id}"));
                    for i in indices {
                        self.objects[i].options.group = Some(id);
                    }
                }
                Command::Ungroup => {
                    for i in indices {
                        self.objects[i].options.group = None;
                    }
                }
                Command::ToggleLock => {
                    let locked = !indices.iter().all(|i| self.objects[*i].options.locked);
                    for i in indices {
                        self.objects[i].options.locked = locked;
                    }
                }
                Command::Front | Command::Back => {
                    let ids: HashSet<_> = indices.into_iter().map(|i| self.objects[i].id).collect();
                    let (mut chosen, mut rest): (Vec<_>, Vec<_>) =
                        self.objects.drain(..).partition(|o| ids.contains(&o.id));
                    if matches!(command, Command::Front) {
                        rest.append(&mut chosen);
                        self.objects = rest;
                    } else {
                        chosen.append(&mut rest);
                        self.objects = chosen;
                    }
                }
                _ => {
                    for i in indices {
                        let o = &mut self.objects[i];
                        if !o.visual_media() {
                            continue;
                        }
                        match command {
                            Command::FlipX => o.options.flip_x = !o.options.flip_x,
                            Command::FlipY => o.options.flip_y = !o.options.flip_y,
                            Command::Grayscale => o.options.grayscale = !o.options.grayscale,
                            Command::Opacity(value) => o.options.opacity = value.clamp(0.0, 1.0),
                            Command::ResetCrop => o.reset_crop(),
                            _ => {}
                        }
                    }
                }
            }
        }
        self.clean_groups();
        self.upgrade_options();
        true
    }
    fn layout(&mut self, indices: &[usize], command: Command, aspect: f32) {
        let included: HashSet<_> = indices.iter().copied().collect();
        let partial: HashSet<_> = self
            .objects
            .iter()
            .enumerate()
            .filter(|(i, _)| !included.contains(i))
            .filter_map(|(_, o)| o.options.group)
            .collect();
        let mut units: Vec<Unit> = Vec::new();
        let mut groups = HashMap::new();
        for &i in indices {
            let o = &self.objects[i];
            if let Some(group) = o.options.group.filter(|g| !partial.contains(g)) {
                let index = *groups.entry(group).or_insert_with(|| {
                    units.push(Unit {
                        indices: vec![],
                        bounds: Rect::NOTHING,
                    });
                    units.len() - 1
                });
                units[index].indices.push(i);
                units[index].bounds = units[index].bounds.union(o.bounds());
            } else {
                units.push(Unit {
                    indices: vec![i],
                    bounds: o.bounds(),
                });
            }
        }
        let bounds = units.iter().map(|u| u.bounds).reduce(Rect::union).unwrap();
        let gap = self.canvas.gap;
        if let Command::Normalize(axis) = command {
            let metric = |b: Rect| match axis {
                0 => b.width(),
                1 => b.height(),
                _ => b.area().sqrt(),
            };
            let target = units.iter().map(|u| metric(u.bounds)).sum::<f32>() / units.len() as f32;
            for unit in &units {
                let scale = (target / metric(unit.bounds).max(0.1)).clamp(0.001, 1000.0);
                for &i in &unit.indices {
                    let o = &mut self.objects[i];
                    let p = unit.bounds.center() + (o.center() - unit.bounds.center()) * scale;
                    o.center = [p.x, p.y];
                    o.size = o.size.map(|v| (v * scale).clamp(0.1, 1e7));
                }
            }
            return;
        }
        if let Command::Align(axis) = command {
            for unit in &units {
                let b = unit.bounds;
                let delta = match axis {
                    0 => Vec2::new(bounds.left() - b.left(), 0.0),
                    1 => Vec2::new(bounds.center().x - b.center().x, 0.0),
                    2 => Vec2::new(bounds.right() - b.right(), 0.0),
                    3 => Vec2::new(0.0, bounds.top() - b.top()),
                    4 => Vec2::new(0.0, bounds.center().y - b.center().y),
                    _ => Vec2::new(0.0, bounds.bottom() - b.bottom()),
                };
                self.move_unit(unit, delta);
            }
            return;
        }
        // Reserve backdrops and their screen-sized headers when packing groups.
        for unit in &mut units {
            if self.objects[unit.indices[0]]
                .options
                .group
                .is_some_and(|g| !partial.contains(&g))
            {
                unit.bounds = group_rect(unit.bounds, self.camera.zoom);
            }
        }
        if matches!(command, Command::Pack) {
            units.sort_by(|a, b| b.bounds.height().total_cmp(&a.bounds.height()));
        }
        let total = units
            .iter()
            .map(|u| (u.bounds.width() + gap) * (u.bounds.height() + gap))
            .sum::<f32>();
        let cell = units.iter().fold(Vec2::ZERO, |s, u| s.max(u.bounds.size()));
        let target = (total * aspect.clamp(0.3, 4.0)).sqrt().max(cell.x);
        let columns = ((units.len() as f32 * aspect.clamp(0.3, 4.0) * cell.y / cell.x.max(0.1))
            .sqrt()
            .ceil() as usize)
            .max(1);
        let mut p = Pos2::ZERO;
        let mut row_height = 0.0_f32;
        let mut placed = Rect::NOTHING;
        for (index, unit) in units.iter().enumerate() {
            match command {
                Command::Grid => {
                    p = Pos2::new(
                        (index % columns) as f32 * (cell.x + gap),
                        (index / columns) as f32 * (cell.y + gap),
                    )
                }
                Command::Pack if p.x > 0.0 && p.x + unit.bounds.width() > target => {
                    p.x = 0.0;
                    p.y += row_height + gap;
                    row_height = 0.0;
                }
                _ => {}
            }
            let r = Rect::from_min_size(p, unit.bounds.size());
            self.move_unit(unit, p - unit.bounds.min);
            placed = placed.union(r);
            row_height = row_height.max(unit.bounds.height());
            match command {
                Command::Column => p.y += unit.bounds.height() + gap,
                _ => p.x += unit.bounds.width() + gap,
            }
        }
        let delta = bounds.center() - placed.center();
        for unit in &units {
            self.move_unit(unit, delta);
        }
    }
    fn move_unit(&mut self, unit: &Unit, delta: Vec2) {
        for &i in &unit.indices {
            self.objects[i].center[0] += delta.x;
            self.objects[i].center[1] += delta.y;
        }
    }
}
