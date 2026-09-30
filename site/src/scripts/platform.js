// The download buttons (components/Download.astro): on a Mac, the Mac download
// comes first and largest, and Windows moves beneath. Both links are already in
// the HTML, so without scripts nothing is missing, only the order differs.
// No framework, no inline styles (the CSP allows none): classes from global.css.
const onMac = /Macintosh|Mac OS X/.test(navigator.userAgent);

if (onMac) {
  for (const box of document.querySelectorAll('[data-dl]')) {
    const win = box.querySelector('[data-os="windows"]');
    const mac = box.querySelector('[data-os="mac"]');
    if (!win || !mac) continue;
    const big = win.className;
    mac.className = big;
    mac.textContent = 'Download for Mac';
    win.className = 'dl-also';
    win.textContent = 'Also available for Windows';
    box.insertBefore(mac, win);
  }
}
