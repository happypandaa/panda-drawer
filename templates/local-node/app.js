const button = document.querySelector('#check');
const result = document.querySelector('#result');
button.addEventListener('click', async () => {
  button.disabled = true;
  result.textContent = '正在连接…';
  try {
    const response = await fetch('/api/hello', { cache: 'no-store' });
    if (!response.ok) throw new Error('request failed');
    const value = await response.json();
    result.textContent = value.message;
  } catch {
    result.textContent = '连接尚未完成，请从平台重新打开应用后重试。';
  } finally { button.disabled = false; }
});
