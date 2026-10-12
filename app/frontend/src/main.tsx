import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { WidgetApp } from './components/widget/WidgetApp';
import { isWidgetWindow } from './services/ipc';
import './styles.css';

const root = document.getElementById('root');
if (!root) throw new Error('Root element not found');

// The same page is loaded into two windows. In the widget's it is the
// widget, on a transparent page, so that only its card shows.
const widget = isWidgetWindow();
if (widget) document.documentElement.classList.add('widget');

ReactDOM.createRoot(root).render(
  <React.StrictMode>{widget ? <WidgetApp /> : <App />}</React.StrictMode>,
);
