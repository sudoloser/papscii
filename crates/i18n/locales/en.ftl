# Papscii UI strings. All app text is keyed here (Android string-resource style).
# Keys are authored with dots in code (e.g. `gallery.card.use`) but stored
# hyphenated because Fluent message identifiers cannot contain dots.

app-name = Papscii
app-tagline = ASCII animated wallpapers

# --- Gallery / empty state ---
gallery-empty-title = Create your first Papscii!
gallery-empty-create = Create
gallery-empty-import = Import from .pscii
gallery-card-use = Use
gallery-card-in-use = In use
gallery-card-edit = Edit
gallery-card-delete = Delete
gallery-card-new = New Papscii
gallery-card-create = Create
gallery-card-by = by { $author }
gallery-card-frames = { $count ->
    [one] { $count } frame
   *[other] { $count } frames
}

# --- Top bar ---
topbar-settings = Settings
topbar-start = Start wallpaper
topbar-stop = Stop wallpaper

# --- Settings screen ---
settings-title = Settings
settings-tab-appearance = Appearance
settings-tab-system = System
settings-tab-about = About
settings-appearance-theme = Theme
settings-appearance-color-mode-dark = Dark
settings-appearance-color-mode-light = Light

# --- System tab ---
settings-system-autostart = Start on boot
settings-system-enabled = Enabled
settings-system-disabled = Disabled
settings-system-enable = Enable
settings-system-disable = Disable
settings-system-status = Service
settings-system-running = Running
settings-system-stopped = Stopped
settings-system-unavailable = The papscii systemd user service is not installed.
settings-system-hint = Stopping clears the wallpaper. Starting resumes the last background used.
settings-system-focus = Keep animating while focused
settings-system-focus-hint = Animation pauses while a window has focus. Checked apps keep animating instead.
settings-system-focus-empty = No open windows right now. Type an app class below.
settings-system-focus-add = Add
settings-system-focus-add-hint = App class, e.g. mpv

# --- About ---
about-version = Version { $version }
about-developer = Developer
about-github = GitHub
about-license = License

# --- Encoding ---
encode-title = New Papscii
encode-field-name = Name
encode-field-video = Video
encode-field-file = File
encode-field-browse = Browse
encode-field-fps = Frames per second
encode-field-density = Density
encode-field-text-size = Text size
encode-field-text-size-hint = 0 = fit screen
encode-field-options = Options
encode-field-color = Include colour
encode-field-transparent = Transparent background
encode-field-brightness = Brightness
encode-field-charset = Character set
encode-field-color-mode = Colour mode
encode-action-start = Create
encode-action-cancel = Cancel
encode-status-working = Encoding…

# --- Editing ---
edit-title = Edit Papscii
edit-field-brightness = Brightness
edit-field-fps = Frames per second
edit-field-text-size = Text size
edit-field-transparent = Transparent background
edit-action-save = Save
edit-action-cancel = Cancel
edit-status-working = Updating…

# --- Native file dialogs ---
dialog-pick-video = Select a video
dialog-pick-pscii = Select a .pscii file
dialog-filter-video = Video
dialog-filter-pscii = Papscii
