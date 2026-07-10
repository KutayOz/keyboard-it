# keyboard-it dmgbuild settings — branded installer window.
# Invoked by packaging/mac/package.sh:
#   python3 -m dmgbuild -s packaging/mac/dmg-settings.py \
#       -D app=dist/keyboard-it.app -D settings_dir=packaging/mac \
#       "keyboard-it" dist/keyboard-it-<version>.dmg
#
# dmgbuild executes this file with `defines` injected into the namespace
# but without `__file__`, so the settings dir comes in via -D settings_dir
# (fallback assumes cwd is the repo root, as package.sh guarantees).
# Icon slot coordinates must match packaging/mac/make_dmg_background.py.
import os.path

HERE = defines.get("settings_dir", "packaging/mac")  # noqa: F821

app = defines.get("app", "dist/keyboard-it.app")  # noqa: F821
appname = os.path.basename(app)

# Volume / image
volume_name = "keyboard-it"
format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
# No hide_extensions: it stamps com.apple.FinderInfo on the bundle root,
# which fails `codesign --verify --strict` (detritus) on the mounted app.

# Window chrome: icon view only, no sidebar/toolbar/statusbar/pathbar.
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
default_view = "icon-view"
show_icon_preview = False

# Background is 660x400 (retina TIFF); window height adds the Finder titlebar.
background = os.path.join(HERE, "dmg-background.tiff")
window_rect = ((200, 140), (660, 428))

# Icon layout (slots kept clear in the background art).
icon_size = 104
text_size = 13
icon_locations = {
    appname: (180, 205),
    "Applications": (480, 205),
}
