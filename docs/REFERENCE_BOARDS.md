# Collection canvases

A collection is an infinite reference canvas. Arrange, rotate and resize images, videos and Markdown cards, write notes and draw annotations. Local images remain references to their originals; browser imports are saved in the collection's own folder.

## Create and open

- Choose **Collections → +**, **Open → New collection…**, or **Actions → New collection from selection…**.
- Enter a name. By default, `Documents\view-calipers\Name\Name.vcboard` stores the canvas, with imported images beside it. The Windows Documents location is used, including redirected Documents folders.
- Click a collection name in the left sidebar. Its canvas occupies the **lower Preview**, with the current file above it. This works with one or two file panes. Drag the horizontal divider to resize the two preview areas.
- Navigate folders, tabs and files freely: the selected collection stays open. Click **empty space below the Collections list** to deselect it. Empty space on the canvas remains available for drawing, panning and selection.
- `.vcboard` files are hidden in file panes. Open collections by name in the sidebar; an explicit file path or command-line argument can still open a standalone legacy board.
- Use **Fullscreen**, F11 with canvas focus, or double-click an image/background to expand the canvas; Escape exits fullscreen.

## Add files

- Drag selected raster images, videos or `.md` / `.markdown` files from either file pane or Windows Explorer onto the canvas. They are added as references, without moving the originals. **Board → Add files…** also works.
- Images inside an archive can be dragged individually or as a group from either pane. The canvas stores the archive path and entry name; keep the archive available when reopening it.
- Drag an image from a web browser onto the canvas, or focus the canvas and paste a copied image with **Ctrl+V**. Image bytes, browser virtual files, HTML image links and direct HTTP(S)/data image links are supported. Imports run in the background and save real image files inside the collection folder, with collision-safe filenames.
- Downloads use the direct image URL without browser cookies. If a site requires login, returns a webpage, or supplies a `blob:` URL, use the browser's **Copy image** command and paste the bitmap.
- File clipboard entries stay references. To own a copy of existing local/archive references, select them and choose **Selection → Copy files into collection folder**. Their position, size and rotation stay intact.

## Video and Markdown

- **Video:** double-click a clip to play or pause it. Select one clip to show its Play/Pause, Mute, Loop, A–B range and full-width scrub timeline above the canvas. Several clips can play independently, including portrait videos; moving, scaling and rotation work during playback. Audio starts muted. Playback uses the existing FFmpeg decoder and GPU YUV renderer.
- Videos reopen paused. Clips pause when they leave the visible canvas or the canvas is closed/hidden. Up to eight clips can play simultaneously; the editor keeps at most 32 loaded video/Markdown objects. Reload retries unavailable sources and updates changed files.
- **Markdown:** the canvas displays a formatted card with headings, lists, emphasis, links and code. Double-click the card or choose **Read Markdown** to open the complete document in a resizable, scrollable reader. This is a read-only view; edit the original file in your text editor and use **Reload** to refresh it.
- Markdown files must be UTF-8 and no larger than 4 MiB. Cards show an excerpt from the first 32 KiB; the reader shows the full document. Inline images on cards appear as alt text. Copying a Markdown source into a collection copies that file, not linked attachments.
- Videos and Markdown are local file references. **Selection → Copy files into collection folder** can make owned copies without changing the layout or original files. Video copies stream to disk rather than loading an entire clip into RAM. Browser imports and archive drops currently support images only.

## Controls

| Action | Control |
| --- | --- |
| Select and move | Click / drag an object; V selects this tool |
| Select several | Shift/Ctrl+click, or drag a box on empty canvas |
| Scale a selection | Drag its bottom-right square handle |
| Rotate a selection | Drag the round handle above it; Shift snaps to 15° |
| Pan | Middle-button drag, Space+drag, or Pan / H |
| Zoom at the pointer | Mouse wheel over the canvas |
| Fit | F or Fit; frames the selection, or all objects if none are selected |
| Text note | Text / T, then click the canvas; double-click a note to edit |
| Draw | Draw / B, then drag; choose stroke color and width in the toolbar |
| Remove objects | Delete while the canvas has focus; source files are preserved |
| Undo / redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| Save | Ctrl+S, Board → Save, or wait for autosave |

**Selection** also provides rotation and scale steps, Bring to front / Send to back, and Relink source. **Overview** shows a compact overview of board objects; click an object to return to the canvas and frame it. Board gestures take precedence over the ordinary preview's wheel-to-browse behavior.

Keyboard actions affect the focused canvas, not files selected in the browser. The canvas stays selected while you browse folders, switch tabs, or change the active pane.

## Reference tools

**Arrange** offers Pack, Grid layout, Horizontal row, Vertical column, edge/center alignment, Same size (width, height or area), and adjustable Gap. These commands operate on selected unlocked objects, or all unlocked objects when nothing is selected. Scaling preserves aspect ratios. A completely selected group is arranged as one block, preserving the relationships between its members.

**Selection → Group** creates a named group and backdrop. Click or drag any member to select/move the group; Alt+click selects an individual member. **Objects & groups** lists the hierarchy, lets you rename groups and select locked objects to unlock them. Groups are flat, not nested. Locked objects are excluded from ordinary canvas selection, moving, resizing, arrangement and deletion; use the object list or right-click to select them. **View → Lock canvas** prevents edits and new imports while allowing pan/zoom. An import already in progress may still finish.

**Selection → Image / video** provides horizontal/vertical reflection, opacity, grayscale and Reset crop. To crop, choose **Crop** (C), then drag a rectangle inside an image or video; release to apply, Escape to cancel. Crops work in the object's rotated coordinate system. Effects are saved in the board, rendered using the existing GPU textures and never overwrite source pixels. Notes and Markdown remain readable in their original colors. **View → Grayscale images & videos** applies a board-wide display override.

**View** also offers Show canvas grid, Snap to grid with an adjustable step, Isolate selection, Focus / return, Fit whole canvas, and Window always on top. Hold Alt while moving to bypass snapping and Shift to constrain movement to one axis. Isolation captures the selected objects until disabled. Focus remembers the previous camera; Z returns to it. The always-on-top setting affects the whole application window and lasts for the current session.

Right-click an image, clip or Markdown card for **Duplicate** and **Open source folder**. Duplicate creates another canvas object referencing the same original; it does not copy the physical file. Open source folder navigates the active file pane, selects the source and scrolls to it. A source inside an archive opens its containing archive folder. Imported browser images open their collection folder. The canvas stays open; a locked file tab opens a new tab as usual. Missing originals produce a message instead of navigating elsewhere. **Remove from canvas** removes objects only. Ctrl+V still imports images from the system clipboard.

| Action | Shortcut with canvas focus |
| --- | --- |
| Duplicate objects | Ctrl+D |
| Group / ungroup | Ctrl+G / Ctrl+Shift+G |
| Lock / unlock selected objects | Alt+L |
| Pack selected objects, or all unlocked | Ctrl+P |
| Align left / right / top / bottom | Ctrl+arrow keys |
| Crop | C, then drag inside the image |
| Flip horizontally / vertically | Alt+Shift+H / Alt+Shift+V |
| Grayscale selected images/videos | Alt+G |
| Focus selection / return | Z |
| Focus previous / next object | Left / Right |

### PureRef feature analysis

The most valuable additions for this media browser are reducing the time spent arranging references, keeping related references together, inspecting composition without modifying originals, and switching between detail and the whole board. These priorities come from PureRef's official [Arrange & align](https://www.pureref.com/handbook/images/organize/), [Images](https://www.pureref.com/handbook/images/), [Features](https://www.pureref.com/handbook/features/) and [Canvas](https://www.pureref.com/handbook/canvas/) documentation, reviewed October 2026.

| Priority | view-calipers implementation |
| --- | --- |
| Arrange references quickly | Pack, grid, row/column, alignment, normalized sizes and gap |
| Organize complex boards | Named groups/backdrops, object list and locks |
| Inspect composition | Nondestructive crop, reflection, opacity and grayscale |
| Stay oriented | Focus/return, previous/next focus, isolation, fit and overview |
| Work alongside a DCC | Always on top, independent window resizing and canvas zoom |
| Keep source access | Right-click reveal in the file pane, portable relative references |

Infinite pan/zoom, notes, drawing, autosave, undo, browser image import and file references were already present. This update adds the tools above rather than attempting complete PureRef compatibility. Nested groups, click-through desktop overlays, pressure-sensitive ink, scene image export and `.pur` import/export remain outside the current scope.

## Saving and storage

Canvases autosave about 650 ms after the last edit, after a drag is released. Writes use a temporary sibling file and an atomic rename. The Saved/Unsaved indicator and save errors appear in the toolbar. Closing a text editor cancels its unapplied draft; press Apply to commit note text.

The library contains `collections.tsv` and one folder per collection. Each folder contains a same-named `.vcboard`, imported images and a small `.collection-id` marker. The marker and canvas file are hidden in app file panes. The JSON stores title, camera, media references, transforms, text and stroke points. Owned files use relative paths; external originals keep their original paths.

**Settings → Collections → Change collection folder…** copies the library to an empty folder and switches to it. The old library is kept. Wait for active image imports to complete before renaming a collection or changing its location. Right-click a collection for **Rename**, **Show collection folder**, or **Remove from library (keep folder)**.

Existing collections and their canvas layouts are migrated from `%LOCALAPPDATA%\ArchView` on first run. Legacy data is kept. Notes, strokes, transforms and image references are preserved. A legacy collection without a canvas receives one from its raster image references; other legacy entries remain in its metadata.

Move a whole collection folder, including its owned files, to keep those references portable. External references still require their original files. Use **Selection → Relink source…** to repair a moved source. Standalone legacy boards offer **Board → Save as…**; managed collections are renamed or relocated through their library controls. If a canvas changes externally while there are unsaved edits, it reports a conflict. On an unsuccessful close/save, recovery copies are attempted under `%LOCALAPPDATA%\ArchView\board-recovery`.

## Current scope

- Raster images, local videos, Markdown cards, text notes and mouse/pen strokes are supported. Strokes currently use a fixed width; tablet pressure and erasing parts of a stroke are not implemented.
- PDF pages, 3D objects, and import/export of other reference-board formats are not supported on the canvas yet.
- Existing version 1 and 2 boards remain readable. Adding video or Markdown requires version 2 (view-calipers 0.4.1+); using the new groups, locks, effects or saved canvas preferences upgrades the board to version 3 (view-calipers 0.5.0+). Earlier versions report an unsupported board instead of silently discarding these properties. Object transforms and references persist; playback, isolation and focus-return history are session-only.
- Thumbnail decoding runs in the background. Visible images request full resolution when large enough on screen, through the existing RAM/GPU photo cache. Each editor holds at most 256 previews and reserves up to 512 MiB for full-resolution textures; at the limit, thumbnails remain visible. The shared photo cache has its own Settings budget.
- Documents are limited to 10,000 objects, 500,000 stroke points and 64 MiB of JSON. Undo keeps up to 50 snapshots with an approximate 32 MiB history budget. These are bounds, not a promise of identical performance for every board size.
