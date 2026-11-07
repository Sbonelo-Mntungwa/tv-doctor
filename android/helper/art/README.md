# TV Nurse artwork

TV Nurse's mark is TV Doctor's television, mirrored: dials and a heart on the
left, and a nurse's cross on the screen where TV Doctor has its heartbeat.

- `nurse.svg` is the source. `nurse.png` is it rendered at 1024 px.
- `res/mipmap-*/ic_launcher.png` and `res/drawable-nodpi/mark.png` are
  `sips -z` copies of `nurse.png`.
- `res/drawable-*/banner.png`, the Android TV home-screen tile (320x180dp), is
  `banner.html` rendered with headless Chrome at scale 1, 1.5 and 2.

```sh
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
"$chrome" --headless=new --hide-scrollbars --force-device-scale-factor=2 \
  --window-size=640,360 --allow-file-access-from-files \
  --screenshot=$PWD/../res/drawable-xxxhdpi/banner.png "file://$PWD/banner.html"
```
