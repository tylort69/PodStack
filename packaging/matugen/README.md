# matugen template

PodStack can follow the colors your matugen setup already generates, so it matches the rest of your desktop and
re-themes the moment you change wallpaper. Add this to `~/.config/matugen/config.toml`:

```toml
[templates.podstack]
input_path = "~/code/PodStack/packaging/matugen/podstack.json"   # or /usr/share/podstack/matugen/podstack.json
output_path = "~/.cache/podstack/matugen-colors.json"
```

With the theme set to **Wallpaper (matugen)** (the default when matugen is installed), PodStack reads
`~/.cache/podstack/matugen-colors.json` when it exists and watches it for changes. Without the template it runs
matugen on your current wallpaper itself (found via `~/.cache/current-wallpaper`, hyprpaper or swww).
