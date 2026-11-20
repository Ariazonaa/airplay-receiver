# Demo video

`demo.h264` is the input for the demo mode, which is used for the screenshots in `docs/images/`:

```
target\release\airplay.exe --demo docs\demo\demo.h264
```

Run it in the project folder (after `cargo build --release`).

The app plays the file in a loop through the normal decoder and renderer, as if a device
named "Demo" were mirroring its screen. No receiver is started and no network ports are opened.

## Origin

The video was made for this project by `scripts/make-demo.ps1`. The script draws every frame
with Pillow and encodes them with ffmpeg/x264. It is not a recording of a real device. The
phone screen is made up: simple coloured tiles with geometric shapes, a clock card that says
"12:30" and "Demo", and a moving light. It contains no third-party icons, logos or screen
content. The clock uses Segoe UI from `C:\Windows\Fonts` and is rendered into the pixels.

The video is licensed under the same license as the rest of the project (GPL-3.0, see
[LICENSE](../../LICENSE)).

## Format

- H.264 High profile, Annex-B byte stream (no container), 590x1280, 60 fps, BT.709 limited range
- 360 frames = a 6-second loop. Frame 0 is the only keyframe, with SPS/PPS in front of it.
- No B-frames, one slice per frame. The demo mode splits the file into frames at each slice.

To regenerate it (Windows, needs `uv` and an ffmpeg with libx264, by default MSYS2 UCRT64):

```
powershell -ExecutionPolicy Bypass -File scripts\make-demo.ps1
```
