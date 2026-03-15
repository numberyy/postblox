(function() {
  'use strict';

  var UPLOAD_PATH = '/inboxes/{id}/messages/upload';

  function initCompose(panel) {
    var inboxId = panel.dataset.inboxId;
    if (!inboxId) return;

    var textarea = panel.querySelector('.compose-body');
    var preview = panel.querySelector('.compose-preview');
    var previewBtn = panel.querySelector('.btn-preview');
    var attachList = panel.querySelector('.compose-attachments');
    var htmlInput = panel.querySelector('input[name="html_body"]');
    var pendingFiles = [];

    // --- Toolbar ---
    var toolbar = panel.querySelector('.compose-toolbar');
    if (toolbar) {
      toolbar.addEventListener('click', function(e) {
        var btn = e.target.closest('[data-action]');
        if (!btn || !textarea) return;
        e.preventDefault();
        var action = btn.dataset.action;
        applyFormat(textarea, action);
      });
    }

    // --- Preview toggle ---
    if (previewBtn && preview && textarea) {
      previewBtn.addEventListener('click', function(e) {
        e.preventDefault();
        var showing = !preview.classList.contains('hidden');
        if (showing) {
          preview.classList.add('hidden');
          textarea.classList.remove('hidden');
          previewBtn.textContent = 'Preview';
        } else {
          preview.innerHTML = mdToHtml(textarea.value);
          preview.classList.remove('hidden');
          textarea.classList.add('hidden');
          previewBtn.textContent = 'Edit';
        }
      });
    }

    // --- On form submit: populate html_body from markdown ---
    var form = panel.querySelector('form');
    if (form && htmlInput && textarea) {
      form.addEventListener('htmx:configRequest', function(e) {
        if (textarea.value.trim()) {
          htmlInput.value = mdToHtml(textarea.value);
        }
      });
      form.addEventListener('submit', function() {
        if (textarea.value.trim()) {
          htmlInput.value = mdToHtml(textarea.value);
        }
      });
    }

    // --- Clipboard paste ---
    if (textarea) {
      textarea.addEventListener('paste', function(e) {
        var items = (e.clipboardData || {}).items;
        if (!items) return;
        for (var i = 0; i < items.length; i++) {
          if (items[i].type.indexOf('image/') === 0) {
            e.preventDefault();
            var file = items[i].getAsFile();
            if (file) uploadFile(file, inboxId, textarea, attachList, pendingFiles);
            return;
          }
        }
      });
    }
  }

  function applyFormat(ta, action) {
    var start = ta.selectionStart;
    var end = ta.selectionEnd;
    var text = ta.value;
    var sel = text.substring(start, end);
    var replacement = '';
    var cursorOffset = 0;

    switch (action) {
      case 'bold':
        replacement = '**' + (sel || 'text') + '**';
        cursorOffset = sel ? 0 : -2;
        break;
      case 'italic':
        replacement = '_' + (sel || 'text') + '_';
        cursorOffset = sel ? 0 : -1;
        break;
      case 'code':
        if (sel.indexOf('\n') >= 0) {
          replacement = '```\n' + (sel || 'code') + '\n```';
          cursorOffset = sel ? 0 : -4;
        } else {
          replacement = '`' + (sel || 'code') + '`';
          cursorOffset = sel ? 0 : -1;
        }
        break;
      case 'link':
        replacement = '[' + (sel || 'text') + '](url)';
        cursorOffset = -1;
        break;
      default:
        return;
    }

    ta.value = text.substring(0, start) + replacement + text.substring(end);
    var pos = start + replacement.length + cursorOffset;
    ta.setSelectionRange(pos, pos);
    ta.focus();
  }

  function uploadFile(file, inboxId, textarea, attachList, pendingFiles) {
    var fd = new FormData();
    var meta = { to: ['placeholder@upload.local'], subject: 'attachment-upload' };
    fd.append('metadata', JSON.stringify(meta));
    fd.append('file', file, file.name || 'pasted-image.png');

    var thumb = document.createElement('div');
    thumb.className = 'compose-thumb';
    thumb.textContent = 'Uploading ' + (file.name || 'image') + '...';
    if (attachList) attachList.appendChild(thumb);

    fetch('/inboxes/' + inboxId + '/messages/upload', {
      method: 'POST',
      body: fd,
    }).then(function(r) {
      if (!r.ok) throw new Error(r.status + ' ' + r.statusText);
      return r.json();
    }).then(function(msg) {
      thumb.textContent = '';
      if (file.type && file.type.indexOf('image/') === 0) {
        var img = document.createElement('img');
        img.src = URL.createObjectURL(file);
        img.className = 'compose-thumb-img';
        thumb.appendChild(img);
      }
      var label = document.createElement('span');
      label.textContent = file.name || 'pasted-image.png';
      thumb.appendChild(label);
      pendingFiles.push(file.name || 'pasted-image.png');
    }).catch(function(err) {
      thumb.textContent = 'Upload failed: ' + err.message;
      thumb.className = 'compose-thumb compose-thumb-err';
    });
  }

  // Minimal markdown → HTML (covers bold, italic, code, links, paragraphs)
  function mdToHtml(md) {
    var lines = md.split('\n');
    var html = [];
    var inCode = false;

    for (var i = 0; i < lines.length; i++) {
      var line = lines[i];

      if (line.match(/^```/)) {
        if (inCode) {
          html.push('</code></pre>');
          inCode = false;
        } else {
          html.push('<pre><code>');
          inCode = true;
        }
        continue;
      }

      if (inCode) {
        html.push(escapeHtml(line));
        html.push('\n');
        continue;
      }

      line = escapeHtml(line);
      // inline formatting
      line = line.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
      line = line.replace(/_(.+?)_/g, '<em>$1</em>');
      line = line.replace(/`(.+?)`/g, '<code>$1</code>');
      line = line.replace(/\[(.+?)\]\((.+?)\)/g, '<a href="$2">$1</a>');

      if (line.trim() === '') {
        html.push('<br>');
      } else {
        html.push('<p>' + line + '</p>');
      }
    }

    if (inCode) html.push('</code></pre>');
    return html.join('\n');
  }

  function escapeHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  document.addEventListener('DOMContentLoaded', function() {
    var panels = document.querySelectorAll('[data-compose]');
    for (var i = 0; i < panels.length; i++) initCompose(panels[i]);
  });
})();
