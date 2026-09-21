import { mount } from 'svelte';
import App from './view/App.svelte';
import './view/styles.css';

mount(App, { target: document.getElementById('app')! });

