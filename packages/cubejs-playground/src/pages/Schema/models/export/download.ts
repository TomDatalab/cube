// Saves a data URL or a Blob as `fileName` through a temporary anchor; a
// Blob's object URL is revoked once the click is handed to the browser.
export function download(dataUrlOrBlob: string | Blob, fileName: string): void {
  const blob = typeof dataUrlOrBlob !== 'string';
  const href = blob ? URL.createObjectURL(dataUrlOrBlob) : dataUrlOrBlob;
  const a = document.createElement('a');
  a.href = href;
  a.download = fileName;
  a.style.display = 'none';
  document.body.appendChild(a);
  try {
    a.click();
  } finally {
    a.remove();
    if (blob) {
      setTimeout(() => URL.revokeObjectURL(href), 0);
    }
  }
}
