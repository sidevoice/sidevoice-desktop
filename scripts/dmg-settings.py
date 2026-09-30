# dmgbuild settings for the Sidevoice .dmg: the window scripts/make-icons.mjs draws (src-tauri/dmg/), with
# the app and Applications where that drawing expects them. No Finder scripting: dmgbuild writes the
# window's .DS_Store itself, so the image comes out the same on a headless CI runner.
#
#   dmgbuild -s scripts/dmg-settings.py -D app=<Sidevoice.app> -D background=<background.tiff> \
#            -D layout=src-tauri/dmg/layout.json -D icon=src-tauri/icons/icon.icns Sidevoice <out.dmg>
import json
import os.path

app = defines["app"]  # noqa: F821 (dmgbuild provides `defines`)
layout = json.load(open(defines["layout"]))  # noqa: F821
name = os.path.basename(app)

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = defines["icon"]  # noqa: F821  the volume's own icon
background = defines["background"]  # noqa: F821  1x + 2x in one TIFF

window_rect = ((200, 140), (layout["width"], layout["height"]))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
include_icon_view_settings = True
arrange_by = None
icon_size = 128
text_size = 13
icon_locations = {name: tuple(layout["app"]), "Applications": tuple(layout["applications"])}
