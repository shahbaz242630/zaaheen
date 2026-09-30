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
