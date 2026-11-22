-- Optional Hyprland (0.57+, Lua config) rules for PodStack. Load it from hyprland.lua:
--   dofile(os.getenv("HOME") .. "/code/PodStack/packaging/hyprland/podstack.lua")
-- PodStack's dialogs are modal transient windows, which Hyprland already floats; these rules center them and give
-- the main window a sensible size when it floats.

hl.window_rule({
    name  = "podstack-floating-center",
    match = { class = "^(io\\.github\\.tylort69\\.PodStack)$", float = true },
    center = true,
})

-- Start PodStack at login (with "Start hidden in tray" on in Settings → Window it goes straight to the tray):
-- hl.on("hyprland.start", function () hl.exec_cmd("podstack") end)
