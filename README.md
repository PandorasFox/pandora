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
