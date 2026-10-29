import cube, {
  CubeApi,
  ErrorResponse,
  HttpTransport,
  ITransportResponse,
  TransportOptions,
} from '@cubejs-client/core';

import { onUnauthorized, workspaceHeaders } from './auth';
import { isSameOrigin } from './helpers';

// HttpTransport that ends the session on a 401 from this server.
class PlaygroundTransport extends HttpTransport {
  public request(apiMethod: string, params: any): ITransportResponse<Response> {
    const response = super.request(apiMethod, params);
    const own = isSameOrigin(this.apiUrl);
    return {
      ...response,
      subscribe: <CBResult>(
        callback: (result: Response | ErrorResponse, resubscribe: () => Promise<CBResult>) => CBResult
      ) => response.subscribe<CBResult>((result, resubscribe) => {
        if (own && result && 'status' in result) {
          onUnauthorized(result);
        }
        return callback(result, resubscribe);
      }),
    };
  }
}

// The Cube API client for the Playground: data requests carry the chosen
// workspace (`X-Cube-Workspace`) when the API is this server.
export function createCubeApi(token: string, apiUrl: string): CubeApi {
  const headers: TransportOptions['headers'] = isSameOrigin(apiUrl) ? workspaceHeaders() : {};
  return cube(token, {
    apiUrl,
    transport: new PlaygroundTransport({ authorization: token, apiUrl, headers }),
  });
}
