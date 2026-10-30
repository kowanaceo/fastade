const en = {
  title: 'Desktop tunnel',
  phases: { off: 'Off', connecting: 'Connecting…', connected: 'Connected', reconnecting: 'Reconnecting…', failed: 'Failed', stopping: 'Stopping…' },
  errors: { auth: 'Sign in first.', key: 'Could not read the SSH key. Remove an encrypted key or pick another.', key_generate: 'Could not create the SSH key.', key_type: 'Only ed25519 keys are supported.', key_in_use: 'This key is registered to another device. Use a different key file.', key_rejected: 'The server rejected the SSH key.', api_unavailable: 'The server does not support tunnels yet.', key_permissions: 'Run chmod 600 on the SSH key.', host_key: 'The server host key changed or could not be verified.', ssh_auth: 'SSH authentication failed. Check that the key is registered.', forward: 'The server refused port forwarding.', ssh_connect: 'Could not connect over SSH. Check your network.', ssh_start: 'Could not start SSH.', sshd: 'The local SSH server (sshd) is not available.', network: 'Network error. Check your connection.', settings: 'Tunnel settings are invalid.', busy: 'The tunnel is busy. Try again shortly.', stopping: 'The tunnel is stopping. Try again shortly.', port_changed: 'The port changed. Restart the tunnel.', sleep_prevention: 'Could not prevent the Mac from sleeping.', unsupported: 'Tunnels are not supported on this system.', fallback: 'Tunnel failed. Try again.' },
};

const ko: typeof en = {
  title: '데스크톱 터널',
  phases: { off: '꺼짐', connecting: '연결 중…', connected: '연결됨', reconnecting: '재연결 중…', failed: '실패', stopping: '종료 중…' },
  errors: { auth: '먼저 로그인하세요.', key: 'SSH 키를 읽을 수 없습니다. 암호화된 키는 쓸 수 없으니 다른 키를 선택하세요.', key_generate: 'SSH 키를 만들지 못했습니다.', key_type: 'ed25519 키만 지원합니다.', key_in_use: '이 키는 다른 기기에 등록돼 있습니다. 다른 키 파일을 사용하세요.', key_rejected: '서버가 SSH 키를 거부했습니다.', api_unavailable: '서버가 아직 터널을 지원하지 않습니다.', key_permissions: 'SSH 키에 chmod 600을 적용하세요.', host_key: '서버 호스트 키가 변경되었거나 확인할 수 없습니다.', ssh_auth: 'SSH 인증에 실패했습니다. 키가 등록되어 있는지 확인하세요.', forward: '서버가 포트 포워딩을 거부했습니다.', ssh_connect: 'SSH로 연결하지 못했습니다. 네트워크를 확인하세요.', ssh_start: 'SSH를 시작하지 못했습니다.', sshd: '로컬 SSH 서버(sshd)를 사용할 수 없습니다.', network: '네트워크 오류입니다. 연결을 확인하세요.', settings: '터널 설정이 올바르지 않습니다.', busy: '터널이 사용 중입니다. 잠시 후 다시 시도하세요.', stopping: '터널을 종료하는 중입니다. 잠시 후 다시 시도하세요.', port_changed: '포트가 변경되었습니다. 터널을 다시 시작하세요.', sleep_prevention: '절전 방지를 설정하지 못했습니다.', unsupported: '이 시스템에서는 터널을 지원하지 않습니다.', fallback: '터널 실패. 다시 시도하세요.' },
};

export function tunnelMessages(locale = navigator.language): typeof en {
  return locale.toLowerCase().startsWith('ko') ? ko : en;
}
