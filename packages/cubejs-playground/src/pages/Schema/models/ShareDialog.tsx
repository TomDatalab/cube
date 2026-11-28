import { useHistory } from 'react-router-dom';

import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';

export type ShareDialogProps = {
  open: boolean;
  onOpenChange(open: boolean): void;
  orgName: string;
  platformAdmin: boolean;
};

// How the model is shared today: through its organization's owners. It
// changes nothing, so it also opens read-only.
export function ShareDialog({
  open, onOpenChange, orgName, platformAdmin,
}: ShareDialogProps) {
  const history = useHistory();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Share model</DialogTitle>
          <DialogDescription>
            {`Models are shared through their organization: every owner of ${orgName} can open and edit this model. One person edits at a time; the others see it read-only.`}
          </DialogDescription>
        </DialogHeader>
        {platformAdmin ? (
          <Button
            variant="link"
            className="tw:self-start tw:px-0"
            onClick={() => {
              onOpenChange(false);
              history.push('/admin/organizations');
            }}
          >
            Manage owners
          </Button>
        ) : (
          <p className="tw:text-sm tw:text-muted-foreground">Ask a platform admin to add or remove owners.</p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
