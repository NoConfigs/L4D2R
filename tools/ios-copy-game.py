"""Copies Left 4 Dead 2 to the L4D2 Rewrite iOS app over USB.

The app reads its game data from Documents/Left 4 Dead 2 (src/ios_main.rs).
This copies the folders the game needs, skipping files already on the phone
with the same size (so an interrupted copy resumes), and adds H.264 copies of
the five retail menu background videos (l4d2_background01..05.mp4 beside
the .bik files). iOS cannot decode Bink; src/menu_video.rs plays these with
AVFoundation, frame for frame the same 1280x720 24 fps video.

Needs: the iPhone plugged in and trusted, iTunes or Apple Devices installed,
and pymobiledevice3 (pip install pymobiledevice3).

    python tools/ios-copy-game.py [--game "C:/.../Left 4 Dead 2"] [--videos-only]
"""
import argparse
import asyncio
import inspect
import os
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_GAME = r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2"
FOLDERS = ["hl2", "update", "left4dead2_dlc1", "left4dead2_dlc2", "left4dead2_dlc3", "left4dead2"]
CHUNK = 8 << 20


async def aw(x):
    return await x if inspect.isawaitable(x) else x


def ffmpeg():
    local = os.path.join(ROOT, ".tools", "ffmpeg", "ffmpeg.exe")
    return local if os.path.isfile(local) else "ffmpeg"


def convert_videos(game, out_dir):
    """Encodes media/l4d2_background0N.bik to .mp4 (H.264, same frames)."""
    made = []
    for n in range(1, 6):
        src = os.path.join(game, "left4dead2", "media", f"l4d2_background{n:02d}.bik")
        if not os.path.isfile(src):
            continue
        dst = os.path.join(out_dir, f"l4d2_background{n:02d}.mp4")
        subprocess.run(
            [ffmpeg(), "-hide_banner", "-loglevel", "error", "-y", "-i", src, "-an",
             "-c:v", "libx264", "-preset", "slow", "-crf", "14", "-pix_fmt", "yuv420p",
             "-profile:v", "high", "-level", "4.1", "-movflags", "+faststart", dst],
            check=True,
        )
        made.append((dst, f"left4dead2/media/l4d2_background{n:02d}.mp4"))
    print(f"Converted {len(made)} background videos", flush=True)
    return made


async def find_bundle(lockdown):
    from pymobiledevice3.services.installation_proxy import InstallationProxyService
    proxy = InstallationProxyService(lockdown=lockdown)
    apps = await aw(proxy.get_apps(application_type="User"))
    ids = sorted(b for b in apps if b.startswith("com.l4d2r.rewrite"))
    if not ids:
        sys.exit("L4D2 Rewrite is not installed on the connected iPhone.")
    # AltStore/Sideloadly append the team id; prefer that one if both exist.
    return ids[-1]


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--game", default=DEFAULT_GAME)
    parser.add_argument("--bundle", help="app bundle id (found automatically)")
    parser.add_argument("--videos-only", action="store_true")
    args = parser.parse_args()
    from pymobiledevice3.lockdown import create_using_usbmux
    from pymobiledevice3.services.house_arrest import HouseArrestService
    lockdown = await aw(create_using_usbmux())
    bundle = args.bundle or await find_bundle(lockdown)
    print(f"App: {bundle}", flush=True)
    phone = await aw(HouseArrestService.create(lockdown, bundle, documents_only=True))
    files = []
    if not args.videos_only:
        for folder in FOLDERS:
            for root, _, names in os.walk(os.path.join(args.game, folder)):
                for name in names:
                    path = os.path.join(root, name)
                    files.append((path, os.path.relpath(path, args.game).replace("\\", "/")))
    temp = tempfile.mkdtemp(prefix="l4d2r-media-")
    files += convert_videos(args.game, temp)
    total = sum(os.path.getsize(p) for p, _ in files)
    done = 0
    started = time.time()
    last = 0.
    made = set()
    print(f"{len(files)} files, {total / 2**30:.2f} GB", flush=True)
    for local, rel in files:
        size = os.path.getsize(local)
        remote = "/Documents/Left 4 Dead 2/" + rel
        try:
            st = await aw(phone.stat(remote))
            if int(st.get("st_size", -1)) == size:
                done += size
                continue
        except Exception:
            pass
        folder = remote.rsplit("/", 1)[0]
        if folder not in made:
            await aw(phone.makedirs(folder))
            made.add(folder)
        handle = await aw(phone.fopen(remote, "w"))
        with open(local, "rb") as f:
            while chunk := f.read(CHUNK):
                await aw(phone.fwrite(handle, chunk))
                done += len(chunk)
                if time.time() - last > 15:
                    last = time.time()
                    rate = done / max(1., time.time() - started)
                    print(f"{done / 2**30:6.2f}/{total / 2**30:.2f} GB  {rate / 2**20:5.1f} MB/s  {rel}", flush=True)
        await aw(phone.fclose(handle))
    print(f"DONE {done / 2**30:.2f} GB in {(time.time() - started) / 60:.1f} min", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
