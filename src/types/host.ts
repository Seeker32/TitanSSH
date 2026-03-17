export interface HostConfig {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth_type: AuthType;
  /** 密码在安全存储中的引用键，不含明文 */
  password_ref?: string;
  private_key_path?: string;
  /** 私钥口令在安全存储中的引用键，不含明文 */
  passphrase_ref?: string;
  remark?: string;
}

/** 保存主机请求，含明文凭据，仅用于提交时传递，不得持久化 */
export interface SaveHostRequest {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth_type: AuthType;
  password?: string;
  private_key_path?: string;
  passphrase?: string;
  remark?: string;
}

export enum AuthType {
  Password = "Password",
  PrivateKey = "PrivateKey",
}
