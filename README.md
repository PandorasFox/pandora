# pandora

pandora is a parallax-scrolling wallpaper daemon for wayland systems.
it is primary intended to be used with [niri](https://github.com/yaLTeR/niri) and bound to its IPC stream,
but I'm open to implementing other compositor IPC agents in the future.

## installing
An AUR package is available as [pandora-git](https://aur.archlinux.org/packages/pandora-git), tracking the `release` branch of this repo.

Otherwise, you can install locally with cargo:

    cargo install --git https://github.com/PandorasFox/pandora

### usage

Make sure you copy the included `sample files/pandora.kdl` to `~/.config/pandora/pandora.kdl`
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
some components (such as the Niri agent) won't be able to start up if an invalid or non-existant image is specified in the config file.

There's live config reloading for playing with animation slowdowns and whatnot. The config should only be reloaded if all images in the config can be loaded (this is not strictly enforced on the initial config load, but will log warnings at runtime when reloading the config file).

## misc notes

* render command will eventually want a bit depth/buffer format option at some point.
  * I have an HDR monitor, so I have a vested interest in testing and implementing this
  once support lands in Smithay.

This is my first rust project in a little while, and my first Wayland/graphics project ever, so feedback on
those aspects is welcome :)
