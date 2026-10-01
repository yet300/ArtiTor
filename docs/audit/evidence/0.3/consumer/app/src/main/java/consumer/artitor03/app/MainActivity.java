package consumer.artitor03.app;
import android.app.Activity;
import android.os.Bundle;
import android.widget.TextView;
import com.yet.tor.ArtiTorClient;
public final class MainActivity extends Activity {
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        ArtiTorClient client = new ArtiTorClient();
        TextView text = new TextView(this);
        text.setText(client.getVersion());
        setContentView(text);
        client.shutdown();
    }
}
