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

## demo

https://github.com/user-attachments/assets/a2a0a3d2-f321-458d-9056-a9ce835fbd9f

## misc notes

(mostly for myself to keep track of minor tidbits)
* my primary monitor 'disconnects' when asleep (unlike my secondary monitor??); restoring this will 'best' be handled at the agent level
    * Due to laptops etc that might have varied displays that can get disconnected for long periods of time, I don't think it makes sense to
    try and handle this inside the thread by waiting for a reconnect that might never arrive.
    * It's also a bit annoying (in the current impl) to wait for Wayland events when we're not mid-animation; this type of event makes more sense
    at the higher level where we'll ideally be handling all these in an automated fashion based on config
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
those aspects is welcome :)
