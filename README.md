# polyfjord3d

This tool is a command line implementation of the COLMAP/GLOMAP pipeline.
- Automatic download of tools, or reuse of the tools existing in PATH.
- Adds the tools to PATH. This allows you to call `polyfjord3d` from any terminal.

# Usage
Run `polyfjord3d -h` in any terminal to get the full help contents.

For videos in your current folder you can run:
- `polyfjord3d vid1.mp4 vid2.mp4 folder/vid3.mp4` - accepts multiple videos
- `polyfjord3d --tool colmap vid1.mp4 vid2.mp4` - this uses colmap instead of the default glomap
- `polyfjord3d vid1.mp4 vid2.mp4 --force` - this forces re-building of the files

> [!note]
> It's important that the videos have different names in order to avoid unwanted overwriting of files.