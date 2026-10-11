# Changelog

## 0.5.0 — 2026-10-11

- Add reference-canvas arrangement tools: pack, grid, row/column, edge/center alignment, equal width/height/area and adjustable spacing. Whole groups retain their internal arrangement.
- Add named groups with backdrops, an Objects & groups window, object/canvas locks, duplication, layer ordering and optional grid snapping.
- Add nondestructive image/video crop, horizontal/vertical flip, opacity and grayscale. Effects reuse GPU textures; originals stay unchanged. Save these properties in canvas format version 3 while continuing to read versions 1 and 2.
- Add focus/return, previous/next focus, isolation, grid visibility and an always-on-top window toggle. Rename the canvas overview button to Overview.
- Add a right-click canvas menu with Duplicate and Open source folder. Reveal and select the original in the active file pane, including entries inside archives, while keeping the canvas open. Scroll the file pane to the revealed item.
- Correct Ctrl+Shift+Z redo handling on the canvas.

## 0.4.1 — 2026-10-10

- Add local video and Markdown objects to collection canvases, through drag-and-drop from either pane, Add files, or New collection from selection.
- Play multiple clips independently with pause, mute, loop, A–B range and scrubbing controls. Preserve portrait geometry, reuse GPU YUV rendering, and pause hidden/offscreen clips.
- Render formatted Markdown cards with a resizable full-document reader and source reload.
- Preserve media references, position, scale and rotation through autosave, reopen, relinking and library relocation. Stream optional owned video/Markdown copies into the collection folder.
- Introduce canvas format version 2 for the new media types while retaining support for existing image-only boards. Keep canvas drag interactions stable when media controls appear.

## 0.4.0 — 2026-10-10

- Make collections persistent canvases in the lower Preview, independent of file panes and tabs. Deselect by clicking empty space below the Collections list.
- Store each collection in Documents/view-calipers/Name/Name.vcboard with its imported images; hide canvas files in the file browser.
- Add a configurable library location with background copying, and preserve legacy collections, layouts, notes and reference paths during migration.
- Import browser image drops and clipboard bitmaps into the collection folder. Support Windows virtual-file, HTML image-link and direct-image drag formats, with background downloading and unique filenames.
- Keep local and archive image drops as references; add a command to copy selected sources into the collection folder while preserving their layout.
- Remove horizontal scrolling from the folders, favorites and collections sidebar. Keep full names without ellipses; resizing the sidebar reveals more text.

## 0.3.1 — 2026-10-10

- Fix dragging selected images from an archive in either pane onto a reference board, including group drags in Thumbnails and Details views.
- Preserve archive path and entry name in `.vcboard` files; load previews and full images through the existing background archive readers without extracting files into user folders.
- Support Create Reference Board from selected archive images, archive relinking, and portable references when browser and editor paths use different Windows path prefixes.

## 0.3.0 — 2026-10-10

- Add `.vcboard` reference boards: editable infinite canvases in Preview and fullscreen.
- Create boards from selected raster images, an existing collection, or Open → New Reference Board.
- Drag image references onto a board; move, rotate and resize objects individually or as a selection.
- Add editable text notes and colored freehand strokes, with Undo/Redo and debounced atomic autosave.
- Keep boards with their tabs and share document edits when the same board is open in both previews.
- Load only visible image previews in the background; reuse the photo pipeline and native GPU textures for close-up viewing.
- Preserve source files, relative references where possible, and recovery copies when a board cannot be saved. Refuse to overwrite external changes silently.

## 0.2.12 — 2026-10-10

- Upload video and scrub frames as YUV420 planes and convert BT.709 limited-range color on the GPU. Normalize source matrix/range in FFmpeg.
- Reuse each preview's plane textures and CPU frame buffers; keep both split previews independent.
- Schedule playback redraws by frame deadlines and decoder availability, replacing the fixed 16 ms polling timer.
- Store shared scrub frames in YUV420, approximately halving payload memory versus RGB24.
- Upload opaque photos directly from decoded RGBA buffers, with GPU mipmaps and byte-budget accounting for the full pyramid. Preserve premultiplied-alpha rendering for transparent images.
- Reserve complete photo texture storage before upload, avoiding expensive driver reallocations while building mip levels.
- Add opt-in GPU correctness checks and synchronized upload/draw benchmarks. Keep the egui/Glow frontend.

## 0.2.11 — 2026-10-10

- Compact pane controls into two adaptive rows: file tools and sorting, then a full-width search field.
- Keep type, scope, regex, saved searches, help, and result paging in the Search options popup on narrow panes; show scope and type inline on wide panes.
- Use consistent vector icons for file actions, new folder, sorting, columns, and filter options, with hover backgrounds and tooltips.
- Improve gallery selection outlines and media badges. Keep thumbnail controls in File actions on very narrow panes.
- Keep scrollbars visible as thin handles, enlarge them on hover, and reserve a gutter so the gallery does not overlap them.

## 0.2.10 — 2026-10-10

- Save named workspaces and restore folders, archives, and collections in every tab of both panes.
- Preserve active panes and tabs, locked tabs, selected files, previews, local filters, thumbnail sizes, details columns, and sort order.
- Restore panel widths, pane and preview dividers, Browse / Compare / Review layout, and split / paired preview preferences.
- Manage snapshots using Workspaces: restore, update, rename, or delete. Stored snapshots contain references only.
- Report unavailable locations and open those tabs as This PC while restoring the rest of the workspace.
- Keep performance settings and custom shortcuts unchanged when restoring a workspace.

## 0.2.9 — 2026-10-10

- Create, rename, and delete virtual reference collections from the sidebar without copying original files.
- Add selected files through Actions → Add to collection, or drag files onto a collection in the sidebar or into its pane.
- Browse mixed media from different folders using the existing thumbnails, details, filtering, and fullscreen previews.
- Drag items within a collection to save a custom order. Use Sort → Collection order to restore it after sorting by metadata.
- Delete in a collection removes references only. Missing originals remain marked as missing; Show in folder opens the source location.
- Store collections and their order across restarts in collections.tsv.

## 0.2.8 — 2026-10-10

- Remember each folder's Details/Thumbnails view, sort direction, thumbnail size, and column layout across navigation and restarts.
- Resize details columns by dragging their right edge; drag column headers to reorder.
- Show or hide columns, reorder them, and restore defaults using the Columns menu. Name remains visible.
- Preserve thumbnail sizes when switching tabs.

## 0.2.7 — 2026-10-09

- Create folders from the pane toolbar, Actions menu, or Ctrl+Shift+N.
- Sort by Name, Type, Size, Created, or Modified in ascending or descending order using clickable details headers or the Sort menu in either view.
- Sorting stays with each tab across navigation, refresh, and view changes. Fullscreen file browsing follows the same order; selection and thumbnails keep their file identity.
- Everything sorting is applied to the complete index before pagination.

## 0.2.6

- Per-pane local filtering with file types, masks, exclusions, OR, regular expressions, size and creation/modification date conditions.
- Background Everything SDK integration: indexed all-drive and recursive folder search, 200-result pages, thumbnails and previews.
- Saved searches, Ctrl+F, show-in-folder navigation, and independent pane/tab search state.
- Filtered fullscreen file navigation keeps original media indices.


## 0.2.5

- Decode full-resolution photos on two shared background workers instead of the UI thread, including images inside archives.
- Preload neighboring photos into a shared, byte-limited RAM LRU cache and reuse GPU textures across panes and tabs.
- Prioritize the latest selected photo, cancel obsolete queued work, and avoid applying stale results after navigation.
- Upload at most one speculative texture per idle UI pass; pause speculative GPU uploads during active input.
- Add Photo performance settings for preloading, RAM/texture budgets, usage and cache clearing. Defaults are 4096 MB RAM on high-memory machines and 1024 MB GPU textures.


## 0.2.4

- Restore a visible Dual panes toggle beside Open in the main toolbar. The active state shows when two file panes are enabled.


## 0.2.3

- Resize the paired fullscreen previews by dragging their divider; double-click to reset to 50/50. The ratio is saved.
- Browse files with a full-width slider below fullscreen previews, independently for each pane.
- Hover or scrub the file slider to see a dynamic cached thumbnail and filename.


## 0.2.2 — Unreleased

- Add persistent Pair fullscreen toggle: split previews open side by side with Pane 1 on the left and Pane 2 on the right.
- Route preview double-clicks through the main viewport, including the second pane; Escape restores the regular layout.
- Change preview wheel behavior to file browsing, with Ctrl+wheel for zoom; each comparison half navigates independently.

## 0.2.1 — Unreleased

- Add mouse rectangle selection in thumbnails and Details; Ctrl / Shift retain existing selections and Escape cancels.
- Add Delete to Actions, context menus, command search and configurable keyboard shortcuts, with a confirmation and Windows Recycle Bin operation.
- Accept multi-file drag-and-drop on folder tree entries, folders in the grid and the other pane; Ctrl copies instead of moving.
- Highlight hovered drop targets, select the dragged file, and release media handles before recycling.

## 0.2.0 — Unreleased

- Redesign the workbench with a unified dark palette, Windows UI font, vector icons, compact navigation and an editable address bar.
- Add Compact and Comfortable density, with persistent panel visibility, widths and divider positions.
- Add Browse, Compare and Review workspaces; Review uses a large preview and horizontal filmstrip.
- Add a searchable keyboard command palette (Ctrl+Shift+P), clear active pane borders and redesigned tabs.
- Reduce duplicated toolbars, move uncommon actions into menus, and place video / PDF transport below the preview.
- Adapt pane toolbars to narrow windows and retain all existing file management and media capabilities.

## 0.1.3 — Unreleased

- Add filesystem copy, cut, paste, duplicate and rename, with Explorer-compatible clipboard data and configurable shortcuts.
- Move or copy items between panes using buttons or drag-and-drop (hold Ctrl to copy).
- Add multi-selection, a Details view with file sizes and local creation timestamps, and placeholders for other file types.
- Add an editable top address bar supporting pasted filesystem paths and direct file selection.
- Run file transfers in the background, avoid destination overwrites, handle cross-drive moves and refresh affected panes and tabs.

## 0.1.2 — Unreleased

- Add a shared, byte-bounded RGB RAM cache across panes and tabs, with file-change-aware keys and LRU eviction.
- Automatically select an 8 GiB cache budget on systems with at least 64 GiB RAM; expose a 0–32 GiB budget in Settings.
- Preload sparse timeline frames in the background, using NVIDIA CUDA decode and GPU resizing where supported, with a software fallback.
- Provide immediate cached drag feedback while retaining precise committed seeks; retain foreground-decoded frames for reverse scrubbing.
- Limit preloading to two jobs, prioritize foreground seeks, reserve 25% of capacity, and stop cache growth under physical memory pressure.
- Add RAM usage / preload indicators and a cache clear button; validate CUDA output, fallback, rotation and cancellation.

## 0.1.1 — Unreleased

- Reuse FFmpeg for neighboring forward scrub positions; cache recent backward frames and discard obsolete seek results.
- Remove PNG compression from video poster and scrub extraction; reuse RGB playback buffers and preview textures.
- Follow the source average frame rate up to 120 fps instead of forcing 24 fps.
- Decode JPEG thumbnails at reduced resolution while retaining EXIF orientation and fallback support.
- Adapt thumbnail worker count to available CPU capacity (1–4 per pane).
- Add reproducible release benchmarks and regression checks for EXIF, frame-rate metadata, and decoder reuse.

## 0.1.0 — 2026-10-07

First public release of view-calipers.

- This PC folder tree, bookmarks, breadcrumbs, and navigation history.
- Independent dual panes with tab creation, duplication, locking, and closing.
- Resizable split preview with simultaneous independent video playback.
- Adjustable, cached thumbnails with folder / archive covers and EXIF orientation.
- Image and comic browsing, video playback and scrubbing, A–B ranges and repeat.
- PDF page timeline and scrubbing; formatted Markdown previews.
- GPU-rendered mesh previews for OBJ, FBX, glTF / GLB, and USD-family formats via Blender.
- Fullscreen viewing and configurable shortcuts / viewing preferences.

See README for external dependencies and current limitations.
