// The Copy buttons (components/CopyBox.astro): copy the box's text exactly, then
// say "Copied" for two seconds. Shown only where the browser can copy.
// No framework, no inline styles (the CSP allows none).
const canCopy = Boolean(navigator.clipboard && window.isSecureContext);

for (const btn of document.querySelectorAll('[data-copy]')) {
  const code = btn.parentElement?.querySelector('code');
  if (!canCopy || !code) continue;
  const label = btn.textContent;
  btn.hidden = false;
  btn.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(code.textContent ?? '');
      btn.textContent = 'Copied';
    } catch {
      btn.textContent = 'Select and copy';
    }
    setTimeout(() => { btn.textContent = label; }, 2000);
  });
}
