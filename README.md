# pandora

pandora is a parallax-scrolling wallpaper daemon for wayland systems.
The goal is for it to be integrated with compositor IPC event streams, so that it can automatically control
the background wallpaper as you navigate your workspaces and windows. [niri](https://github.com/yaLTeR/niri) is
the intended use here, but there's no reason why this couldn't be integrated with swaywm or another compositor to
enable a sliding effect as you switch workspaces - it just won't match with niri's layout paradigm as well.

implementation is still in-progress.

a side-goal is to also add screen locking functionality to this. Theoretically, it shouldn't be that hard to
have the daemon spawn some extra threads for the lockscreen surfaces & another to handle inputs (?).
I poked at all of that for i3lock-color a decade ago, and things were wayyy shittier then, right?

## installing
    cd daemon && cargo install --path .

### usage
I recommend executing this with a systemd user unit file (I know, I know). A sample service file is included in the repo:

    cp pandora.service ~/.config/systemd/user/pandora.service && systemctl --user start pandora

I recommend following [niri's example systemd setup](https://github.com/YaLTeR/niri/wiki/Example-systemd-Setup) to leverage `niri.service.wants` if you have multiple compositors installed, and only want to use this with niri at the moment.

## misc notes

(mostly for myself to keep track of minor tidbits)
* render command will eventually want a bit depth/buffer format option at some point (when we have the agent thread that can/will parse
that from its wayland event stream)
* still need to take a fine-toothed comb to the render threads and rework to better use Result<> returns
  * there's .end() now to make sure render threads release buffers etc on their way out
  * response handling in general needs more love
  * maybe need an Error enum that's [unrecoverable, invalid command, LogicFlowError] type shit to differentiate this and make life easier
	* this would map over well to the daemonerrors actually
* [session lock nonsense](https://wayland.app/protocols/ext-session-lock-v1) honestly seems spookily straightforward for letting us reuse the
existing (dma)bufs, just need to implement the input handling etc in its own bespoke thread

This is my first rust project in a little while, and my first Wayland/graphics project ever, so feedback on
those aspects is welcome. I still need to do.... a few different refactorings before adding more features.
