//! Every icon the interface draws, listed once. Each is an SVG: fastframe's
//! shared Lucide files (`lucide "name"`) or ours in `assets/icons/`. To add
//! one, drop its SVG there and add a variant; to swap one, change its file.
//! A missing file fails the build.

use egui::{Color32, Image, Ui, WidgetText};

fastframe_icons::icons! {
    pub enum Icon {
        prefix: "tectec-icon-",
        directory: "../assets/icons/",
        // Shared with the fastframe apps.
        Check => lucide "check",
        ChevronDown => lucide "chevron-down",
        ChevronLeft => lucide "chevron-left",
        ChevronRight => lucide "chevron-right",
        ChevronUp => lucide "chevron-up",
        CircleCheck => lucide "circle-check",
        CircleX => lucide "circle-x",
        Clock => lucide "clock",
        Copy => lucide "copy",
        Eye => lucide "eye",
        Info => lucide "info",
        Maximize => lucide "maximize-2",
        Minus => lucide "minus",
        Moon => lucide "moon",
        PanelLeft => lucide "panel-left",
        Pencil => lucide "pencil",
        Play => lucide "play",
        Plus => lucide "plus",
        Search => lucide "search",
        Settings => lucide "settings",
        Sun => lucide "sun",
        Trash => lucide "trash-2",
        X => lucide "x",
        // Ours.
        ArrowDown => "arrow-down",
        ArrowLeft => "arrow-left",
        ArrowRight => "arrow-right",
        ArrowUp => "arrow-up",
        ArrowUpDown => "arrow-up-down",
        Bold => "bold",
        BookOpenText => "book-open-text",
        Braces => "braces",
        ChevronsLeft => "chevrons-left",
        CircleDot => "circle-dot",
        ClipboardPaste => "clipboard-paste",
        Code => "code",
        Columns => "columns-2",
        Crosshair => "crosshair",
        EllipsisVertical => "ellipsis-vertical",
        File => "file",
        FileCode => "file-code",
        FileImage => "file-image",
        FilePen => "file-pen",
        FilePlus => "file-plus",
        FileText => "file-text",
        Files => "files",
        FitPage => "scan",
        FitWidth => "move-horizontal",
        Folder => "folder",
        FolderOpen => "folder-open",
        FolderPlus => "folder-plus",
        Heading => "heading",
        History => "history",
        Image => "image",
        Images => "images",
        Italic => "italic",
        Keyboard => "keyboard",
        Languages => "languages",
        Library => "library",
        List => "list",
        ListTree => "list-tree",
        Notebook => "notebook",
        NotebookPen => "notebook-pen",
        Package => "package",
        Pin => "pin",
        Quote => "quote",
        Save => "save",
        Share => "share",
        Sigma => "sigma",
        SpellCheck => "spell-check",
        Square => "square",
        Star => "star",
        Table => "table",
        Tag => "tag",
        TriangleAlert => "triangle-alert",
        Type => "type",
        Undo => "undo-2",
        Monitor => "monitor",
        Zap => "zap",
        ZapOff => "zap-off",
        ZoomIn => "zoom-in",
        ZoomOut => "zoom-out",
    }
}

/// Icons next to body text.
pub const SIZE: f32 = 14.0;

impl Icon {
    /// At body-text size, in `color`.
    pub fn tinted(self, color: Color32) -> Image<'static> {
        self.image(color, SIZE)
    }

    /// For buttons: the button's own text colour replaces this white.
    pub fn plain(self) -> Image<'static> {
        self.image(Color32::WHITE, SIZE)
    }
}

/// A button with an icon and a text, the icon following the text's colour
/// (hover, disabled).
pub fn button<'a>(icon: Icon, text: impl Into<WidgetText>) -> egui::Button<'a> {
    egui::Button::new((icon.plain(), text.into())).image_tint_follows_text_color(true)
}

/// A button with only an icon.
pub fn icon_button<'a>(icon: Icon) -> egui::Button<'a> {
    let side = crate::widgets::ICON_BUTTON;
    egui::Button::new(icon.plain()).image_tint_follows_text_color(true).min_size(egui::vec2(side, side))
}

/// A selectable entry with an icon and a text.
pub fn selectable<'a>(selected: bool, icon: Icon, text: impl Into<WidgetText>) -> egui::Button<'a> {
    egui::Button::selectable(selected, (icon.plain(), text.into())).image_tint_follows_text_color(true)
}

/// An icon followed by a text, in the text's colour (`None`: the normal one).
pub fn label(ui: &mut Ui, icon: Icon, text: impl Into<WidgetText>, color: Option<Color32>) -> egui::Response {
    let color = color.unwrap_or_else(|| ui.visuals().text_color());
    let text: WidgetText = text.into();
    ui.add(egui::AtomLayout::new((icon.tinted(color), text.color(color))))
}

/// Just an icon, in `color`, at `size`.
pub fn show(ui: &mut Ui, icon: Icon, color: Color32, size: f32) -> egui::Response {
    ui.add(icon.image(color, size))
}

/// `icon` in `color`, centred in `rect`: over a widget already laid out.
pub fn show_at(ui: &Ui, icon: Icon, rect: egui::Rect, color: Color32) {
    icon.image(color, SIZE).paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(SIZE, SIZE)));
}
