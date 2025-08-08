# pandora

pandora is a parallax-scrolling wallpaper daemon for wayland systems.
it is primary intended to be used with [niri](https://github.com/yaLTeR/niri) and bound to its IPC stream,
but I'm open to implementing other compositor IPC agents once it is feature-complete.

> [!NOTE]
> pandora is functional, performant, and generally usable. New feature extensions (such as lockscreen functionality) are still being implemented.

## installing
    cargo install

### usage
Make sure you copy the included `sample files/pandora.kdl` to `~/.config/pandora.kdl`
(or `$XDG_CONFIG_HOME/pandora.kdl`), and edit it to reflect your outputs
and desired wallpapers. It has some placeholder values of the various options
(a few of which, like triggers and lockscreen state, are to-be-implemented).

I recommend executing this with a systemd user unit file. A sample service file is included in the repo:

    cp 'sample files/pandora.service' ~/.config/systemd/user/pandora.service && systemctl --user enable --now pandora

You can always check the logs with `journalctl -ft pandora` when running as a systemd user unit.

I recommend following [niri's example systemd setup](https://github.com/YaLTeR/niri/wiki/Example-systemd-Setup)
to leverage `niri.service.wants/` if you have multiple compositors installed, and only want to use this with niri at the moment.

You will also need the following in your niri config:

    layer-rule {
      match namespace="^pandora$"
      place-within-backdrop true
    }

## considerations

Due to image geometry being critical for Pandora's many threads to operate across the board,
some components (such as the Niri agent) won't be able to start up if an invalid or non-existant image is specified in
the config file.

Changing an output mode/resolution during runtime Doesn't Crash, but still needs some poking at to make it less jank (e.g. restarting the threads in-place leads to missized images sometimes?). Output plug/unplug events work fine though :)

The config file will live-reload if-and-only-if it can successfully (pre)load every image in the config file, which should make this easier.

## misc notes

(mostly for myself to keep track of minor tidbits)
* render command will eventually want a bit depth/buffer format option at some point
  * outputs watcher thread will need to rig up a callback for the wl_shm (or dma?) object .format event => check available formats there
* [session lock nonsense](https://wayland.app/protocols/ext-session-lock-v1) is pretty straightforward, just need to go write a generic lock thread
that handles the lockscreen surfaces all in one thread (e.g. not using the existing render thread logic) for some mild separation

This is my first rust project in a little while, and my first Wayland/graphics project ever, so feedback on
those aspects is welcome. I still need to do.... a few different refactorings before adding more features.
