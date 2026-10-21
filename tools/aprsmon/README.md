# APRS monitor (Xeon)

The RTL-SDR on 144.390 MHz into Direwolf, receive only, always on.

- Every decoded frame goes to a daily CSV log in `~/aprsmon/logs`.
- Live frames are on KISS/TCP port 8001.
- It never transmits.

## Install (once, on the Xeon)

```
sudo pacman -S rtl-sdr usbutils
yay -S direwolf
echo 'blacklist dvb_usb_rtl28xxu' | sudo tee /etc/modprobe.d/rtl-sdr-blacklist.conf
sudo modprobe -r dvb_usb_rtl28xxu
mkdir -p ~/aprsmon && cp ~/code/rt950pro/ferrite/tools/aprsmon/{run.sh,direwolf.conf} ~/aprsmon/
sudo cp ~/code/rt950pro/ferrite/tools/aprsmon/99-rtl-sdr.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
sudo cp ~/code/rt950pro/ferrite/tools/aprsmon/aprsmon.service /etc/systemd/system/
sudo systemctl enable --now aprsmon
```

The service runs from `~/aprsmon`, not the checkout, so an rsync of the repo doesn't restart it.

## Check

- `rtl_test -t`: the dongle is found.
- `journalctl -u aprsmon -f`: decoded frames as they come.
- `ls ~/aprsmon/logs`: one CSV a day.

## Notes

- **Frequency:** 144.390 is North America's; set `APRS_FREQ` in the service for elsewhere.
- **Tuning:** `PPM` corrects the dongle's crystal and `GAIN` sets the gain; tune them if frames are rare.
- **Bench tests:** `tools/bench.py`'s SDR checks need the dongle on the Mac. Move it back for a TX bench run.
- **iGate:** to forward what it hears to aprs.fi, see the commented lines in `direwolf.conf`.
