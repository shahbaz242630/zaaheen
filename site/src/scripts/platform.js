// The download buttons (components/Download.astro): on a Mac, the Mac button
// comes first. Both buttons are already in the HTML, the same style, so without
// scripts nothing is missing, only the order differs.
// No framework, no inline styles (the CSP allows none).
const onMac = /Macintosh|Mac OS X/.test(navigator.userAgent);

if (onMac) {
  for (const box of document.querySelectorAll('[data-dl]')) {
    const win = box.querySelector('[data-os="windows"]');
    const mac = box.querySelector('[data-os="mac"]');
    if (!win || !mac) continue;
    box.insertBefore(mac, win);
  }
}

// The connect guides' Windows | Mac switch (components/ConnectSteps.astro):
// Windows is checked in the HTML; on a Mac, the Mac tab is chosen instead.
if (onMac) {
  for (const radio of document.querySelectorAll('[data-os-switch] [data-os-mac]')) radio.checked = true;
}
