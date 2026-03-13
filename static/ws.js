(function() {
  var backoff = 1000, maxBackoff = 30000, ws;

  function badge(href, color) {
    var link = document.querySelector('a[href*="' + href + '"]');
    if (!link || link.querySelector('.ws-badge')) return;
    var dot = document.createElement('span');
    dot.className = 'ws-badge';
    dot.style.cssText = 'display:inline-block;width:8px;height:8px;border-radius:50%;background:' + color + ';margin-left:6px;';
    link.appendChild(dot);
  }

  function clearBadge(href) {
    if (location.pathname.indexOf(href) === -1) return;
    var b = document.querySelectorAll('a[href*="' + href + '"] .ws-badge');
    b.forEach(function(el) { el.remove(); });
  }

  function connect() {
    var proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(proto + '//' + location.host + '/dashboard/ws');

    ws.onopen = function() { backoff = 1000; };

    ws.onmessage = function(e) {
      try {
        var msg = JSON.parse(e.data);
        if (msg.event === 'approval.requested') badge('/approvals', '#e74c3c');
        if (msg.event === 'message.received') badge('/inboxes', '#3498db');
      } catch (_) {}
    };

    ws.onclose = function() { retry(); };
    ws.onerror = function() { ws.close(); };
  }

  function retry() {
    setTimeout(connect, backoff);
    backoff = Math.min(backoff * 2, maxBackoff);
  }

  clearBadge('/approvals');
  clearBadge('/inboxes');
  connect();
})();
