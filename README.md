# pandora

pandora is a wallpaper daemon for wayland systems. The goal is for it to be integrated with compositor IPC
event streams, so that it can automatically control the background wallpaper as you navigate your workspaces and
windows. [niri](https://github.com/yaLTeR/niri) is the intended use here, but there's no reason why this couldn't
be integrated with swaywm or another compositor to enable a sliding effect as you switch workspaces - it just won't
match with niri's layout paradigm as well.

implementation is still very in-progress! presently, it's just shy of actually loading static wallpaper images, but
the basic functionality (from loading images to a central shared table, to IPC plumbing, basic commands, and the
general daemon structure) is all there. the minimum functional product is still a while off, but I don't forsee
any real blockers - I mostly just need to keep chipping away at implementation and testing :)

a side-goal is to also add screen locking functionality to this. Theoretically, it shouldn't be that hard to
have the daemon spawn some extra threads for the lockscreen surfaces & another to handle inputs (?).
I poked at all of that for i3lock-color a decade ago, and things were wayyy shittier then, right?

## demo

https://github.com/user-attachments/assets/a2a0a3d2-f321-458d-9056-a9ce835fbd9f

## misc notes

(mostly for myself to keep track of minor tidbits)

* this is *not vram friendly* at the moment, due to wanting to load images that are bigger than the display resolution.
I believe this can maybe be addressed using [dmabufs](https://wayland.app/protocols/linux-dmabuf-v1#zwp_linux_dmabuf_v1)
alongside texture compression (?), but I wouldn't count on it.
    * i have 24GiB of vram so this is admittedly very low on my list of priorities
* my primary monitor 'disconnects' when asleep (unlike my secondary monitor??); restoring this will 'best' be handled at the agent level
    * Due to laptops etc that might have varied displays that can get disconnected for long periods of time, I don't think it makes sense to
    try and handle this inside the thread by waiting for a reconnect that might never arrive.
    * It's also a bit annoying (in the current impl) to wait for Wayland events when we're not mid-animation; this type of event makes more sense
    at the higher level where we'll ideally be handling all these in an automated fashion based on config
* render command will eventually want a bit depth/buffer format option at some point (when we have the agent thread that can/will parse
that from its wayland event stream)
* i am Learning A Lot about wayland protocol nonsense in real-time, and de-rusting my rust, so I plan to do a buncha cleanup before tackling
the agent thread
* [session lock nonsense](https://wayland.app/protocols/ext-session-lock-v1) honestly seems spookily straightforward for letting us reuse the
existing (dma)bufs, just need to implement the input handling etc in its own bespoke thread
